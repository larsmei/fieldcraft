//! Positive-definite factorization of a free-dof matrix that nested dissection has already
//! numbered.
//!
//! [`FactorMethod::Native`] is FieldCraft's own supernodal Cholesky. [`FactorMethod::Faer`] asks
//! faer to factor that same numbering (`Identity`, so it does not run AMD) and to use its
//! supernodal kernel. faer is optional: a build without the feature, including the browser,
//! refuses it. Indefinite systems stay on [`crate::ldlt`].

use crate::cholesky::{Factor, FactorError, UpperColumns};
use crate::sparse::Csr;
use crate::{Error, FactorMethod, Limits, Result};

/// A Cholesky factor of a nested-dissection matrix. The native factor and, when this build has
/// faer, faer's factor. Opaque so a faer value does not leak into the public variant.
#[derive(Debug, Clone)]
pub struct SpdFactor {
    inner: Inner,
}

#[derive(Debug, Clone)]
enum Inner {
    Native(Factor),
    #[cfg(feature = "faer")]
    Faer(FaerFactor),
}

impl SpdFactor {
    pub(crate) fn native(factor: Factor) -> SpdFactor {
        SpdFactor { inner: Inner::Native(factor) }
    }

    pub(crate) fn n(&self) -> usize {
        match &self.inner {
            Inner::Native(f) => f.n,
            #[cfg(feature = "faer")]
            Inner::Faer(f) => f.n,
        }
    }

    /// Entries stored for the factor. faer's count includes supernode padding, so it can be
    /// larger than the native factor of the same matrix.
    pub(crate) fn entries(&self) -> usize {
        match &self.inner {
            Inner::Native(f) => f.entries(),
            #[cfg(feature = "faer")]
            Inner::Faer(f) => f.entries,
        }
    }

    /// Solve A x = b in place.
    pub(crate) fn solve(&self, b: &mut [f64]) -> Result<()> {
        match &self.inner {
            Inner::Native(f) => f.solve(b),
            #[cfg(feature = "faer")]
            Inner::Faer(f) => f.solve(b),
        }
    }
}

/// `factor` asked for faer and this build has no faer kernel.
pub(crate) fn unavailable() -> Error {
    Error::NotSupported("factor \"faer\" is not in this build (the browser has no faer kernel); use \"native\"".into())
}

/// Factor `a`. The caller has already numbered it by nested dissection.
pub(crate) fn factor(a: &Csr, limits: &Limits) -> std::result::Result<SpdFactor, FactorError> {
    match limits.factor {
        FactorMethod::Native => Factor::new(&UpperColumns::from_symmetric(a), limits.max_factor_entries).map(SpdFactor::native),
        FactorMethod::Faer => faer_or_unavailable(a, limits.max_factor_entries),
    }
}

#[cfg(not(feature = "faer"))]
fn faer_or_unavailable(a: &Csr, max_entries: usize) -> std::result::Result<SpdFactor, FactorError> {
    let _ = (a, max_entries);
    Err(FactorError::Unavailable)
}

#[cfg(feature = "faer")]
fn faer_or_unavailable(a: &Csr, max_entries: usize) -> std::result::Result<SpdFactor, FactorError> {
    // Nothing to factor: the native empty factor is the same answer, and faer would divide a
    // zero flop count by a zero factor size.
    if a.n == 0 {
        return Factor::new(&UpperColumns::from_symmetric(a), max_entries).map(SpdFactor::native);
    }
    FaerFactor::new(a, max_entries).map(|f| SpdFactor { inner: Inner::Faer(f) })
}

#[cfg(feature = "faer")]
#[derive(Clone)]
struct FaerFactor {
    n: usize,
    entries: usize,
    state: std::sync::Arc<FaerState>,
}

#[cfg(feature = "faer")]
struct FaerState {
    symbolic: faer::sparse::linalg::cholesky::SymbolicCholesky<usize>,
    values: Vec<f64>,
}

#[cfg(feature = "faer")]
impl std::fmt::Debug for FaerFactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FaerFactor").field("n", &self.n).field("entries", &self.entries).finish_non_exhaustive()
    }
}

#[cfg(feature = "faer")]
fn faer_alloc(error: faer::sparse::FaerError, entries: usize) -> FactorError {
    match error {
        faer::sparse::FaerError::OutOfMemory => FactorError::OutOfMemory { entries },
        _ => FactorError::Internal(format!("faer factorization: {error}")),
    }
}

#[cfg(feature = "faer")]
impl FaerFactor {
    fn new(a: &Csr, max_entries: usize) -> std::result::Result<FaerFactor, FactorError> {
        use faer::linalg::cholesky::llt::factor::{LltError as NumericLlt, LltRegularization};
        use faer::sparse::linalg::SupernodalThreshold;
        use faer::sparse::linalg::cholesky::{CholeskySymbolicParams, SymmetricOrdering, factorize_symbolic_cholesky};
        use faer::sparse::{CreationError, SparseColMat, Triplet};

        let mut triplets = Vec::new();
        if triplets.try_reserve(a.val.len()).is_err() {
            return Err(FactorError::OutOfMemory { entries: a.val.len() });
        }
        // Upper triangle. Symbolic analysis reads that half, and with no reordering it is not
        // transposed first. The supernodal numeric kernel is then handed the lower half.
        for i in 0..a.n {
            let (cols, vals) = a.row(i);
            for (&j, &v) in cols.iter().zip(vals) {
                if i <= j {
                    triplets.push(Triplet::new(i, j, v));
                }
            }
        }
        let matrix = SparseColMat::<usize, f64>::try_new_from_triplets(a.n, a.n, &triplets).map_err(|e| match e {
            CreationError::Generic(err) => faer_alloc(err, triplets.len()),
            CreationError::OutOfBounds { row, col } => FactorError::Internal(format!("faer triplet ({row}, {col}) is outside the matrix")),
        })?;
        let symbolic = factorize_symbolic_cholesky(
            matrix.symbolic(),
            faer::Side::Upper,
            SymmetricOrdering::Identity,
            CholeskySymbolicParams { supernodal_flop_ratio_threshold: SupernodalThreshold::FORCE_SUPERNODAL, ..CholeskySymbolicParams::default() },
        )
        .map_err(|e| faer_alloc(e, triplets.len()))?;
        // Threshold 0 still leaves a degenerate pattern simplicial. Both kernels solve it;
        // every matrix with a positive flop count is supernodal.
        let entries = symbolic.len_val();
        if entries > max_entries {
            return Err(FactorError::TooLarge { entries, limit: max_entries });
        }
        let mut values = Vec::new();
        if values.try_reserve_exact(entries).is_err() {
            return Err(FactorError::OutOfMemory { entries });
        }
        values.resize(entries, 0.0);
        let par = faer::Par::rayon(fieldcraft_parallel::current_threads().max(1));
        let scratch = symbolic.factorize_numeric_llt_scratch::<f64>(par, Default::default());
        let mut buffer = faer::dyn_stack::MemBuffer::try_new(scratch).ok().ok_or(FactorError::OutOfMemory { entries })?;
        symbolic
            .factorize_numeric_llt(
                &mut values,
                matrix.as_ref(),
                faer::Side::Upper,
                LltRegularization::<f64>::default(),
                par,
                faer::dyn_stack::MemStack::new(&mut buffer),
                Default::default(),
            )
            .map_err(|e| match e {
                NumericLlt::NonPositivePivot { index } => FactorError::NotPositiveDefinite { column: index },
            })?;
        Ok(FaerFactor { n: a.n, entries, state: std::sync::Arc::new(FaerState { symbolic, values }) })
    }

    fn solve(&self, b: &mut [f64]) -> Result<()> {
        use faer::sparse::linalg::cholesky::LltRef;

        if b.len() != self.n {
            return Err(Error::Internal("right-hand side length".into()));
        }
        let par = faer::Par::rayon(fieldcraft_parallel::current_threads().max(1));
        let scratch = self.state.symbolic.solve_in_place_scratch::<f64>(1, par);
        let mut buffer = faer::dyn_stack::MemBuffer::try_new(scratch)
            .ok()
            .ok_or_else(|| Error::Limit(format!("not enough memory to solve with the faer factor ({} equations)", self.n)))?;
        let llt = LltRef::new(&self.state.symbolic, &self.state.values);
        let rhs = faer::MatMut::from_column_major_slice_mut(b, self.n, 1);
        llt.solve_in_place_with_conj(faer::Conj::No, rhs, par, faer::dyn_stack::MemStack::new(&mut buffer));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csr(rows: &[Vec<(usize, f64)>]) -> Csr {
        let mut ptr = vec![0];
        let mut idx = Vec::new();
        let mut val = Vec::new();
        for mut row in rows.iter().cloned() {
            row.sort_by_key(|e| e.0);
            for (c, v) in row {
                idx.push(c);
                val.push(v);
            }
            ptr.push(idx.len());
        }
        Csr { n: rows.len(), ptr, idx, val }
    }

    /// Symmetric tridiagonal, both triangles stored: 4 on the diagonal, 1 beside it.
    fn spd(n: usize) -> Csr {
        let rows = (0..n)
            .map(|i| {
                let mut row = vec![(i, 4.0)];
                if i > 0 {
                    row.push((i - 1, 1.0));
                }
                if i + 1 < n {
                    row.push((i + 1, 1.0));
                }
                row
            })
            .collect::<Vec<_>>();
        csr(&rows)
    }

    fn faer_limits(max_factor_entries: usize) -> Limits {
        Limits { factor: FactorMethod::Faer, max_factor_entries, ..Limits::default() }
    }

    #[cfg(not(feature = "faer"))]
    #[test]
    fn faer_is_refused_without_the_feature() {
        let err = factor(&spd(3), &faer_limits(usize::MAX)).unwrap_err();
        assert_eq!(err, FactorError::Unavailable);
        assert!(unavailable().to_string().contains("faer"), "{}", unavailable());
    }

    #[cfg(feature = "faer")]
    #[test]
    fn faer_matches_native_on_the_given_order() {
        let a = spd(8);
        let native = factor(&a, &Limits::default()).unwrap();
        let fast = factor(&a, &faer_limits(usize::MAX)).unwrap();
        assert!(fast.entries() > 0, "faer reports its stored values, padding included");
        let rhs: Vec<f64> = (0..a.n).map(|i| (i as f64 * 0.37).sin() + 0.2).collect();
        let mut x = rhs.clone();
        let mut y = rhs.clone();
        native.solve(&mut x).unwrap();
        fast.solve(&mut y).unwrap();
        for (p, q) in x.iter().zip(&y) {
            assert!((p - q).abs() < 1e-8, "{x:?} vs {y:?}");
        }
        let mut ay = vec![0.0; a.n];
        a.mul(&y, &mut ay);
        for (p, q) in ay.iter().zip(&rhs) {
            assert!((p - q).abs() < 1e-8, "residual {ay:?} vs {rhs:?}");
        }

        let empty = Csr { n: 0, ptr: vec![0], idx: Vec::new(), val: Vec::new() };
        let zero = factor(&empty, &faer_limits(10)).unwrap();
        assert_eq!(zero.entries(), 0);
        zero.solve(&mut []).unwrap();

        let one = csr(&[vec![(0, 4.0)]]);
        assert!(matches!(factor(&one, &faer_limits(0)), Err(FactorError::TooLarge { .. })));

        // Positive first pivot, negative second: the free-dof column, not a fill-reducing renumber.
        let indefinite = csr(&[vec![(0, 4.0), (1, 1.0)], vec![(0, 1.0), (1, -3.0)]]);
        assert_eq!(factor(&indefinite, &faer_limits(usize::MAX)).unwrap_err(), FactorError::NotPositiveDefinite { column: 1 });
    }
}
