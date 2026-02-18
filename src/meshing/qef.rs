use crate::world::chunk::VOXEL_SCALE;

/// QEF (Quadratic Error Function) solver for dual contouring.
///
/// Finds the vertex position that minimizes the sum of squared distances
/// to all constraint planes. Uses eigendecomposition of the 3x3 symmetric
/// ATA matrix for numerical stability.

/// 3x3 symmetric matrix stored as upper triangle [a00, a01, a02, a11, a12, a22].
#[derive(Clone, Copy)]
struct Sym3x3 {
    m: [f64; 6],
}

impl Sym3x3 {
    const ZERO: Self = Self { m: [0.0; 6] };

    #[inline]
    fn get(&self, r: usize, c: usize) -> f64 {
        let (r, c) = if r > c { (c, r) } else { (r, c) };
        match (r, c) {
            (0, 0) => self.m[0],
            (0, 1) => self.m[1],
            (0, 2) => self.m[2],
            (1, 1) => self.m[3],
            (1, 2) => self.m[4],
            (2, 2) => self.m[5],
            _ => unreachable!(),
        }
    }

    fn add_outer_product(&mut self, n: [f64; 3]) {
        self.m[0] += n[0] * n[0];
        self.m[1] += n[0] * n[1];
        self.m[2] += n[0] * n[2];
        self.m[3] += n[1] * n[1];
        self.m[4] += n[1] * n[2];
        self.m[5] += n[2] * n[2];
    }
}

/// Jacobi eigendecomposition for 3x3 symmetric matrix.
/// Returns (eigenvalues, eigenvectors_as_columns).
fn eigen_decompose(mat: &Sym3x3) -> ([f64; 3], [[f64; 3]; 3]) {
    // Work on a mutable dense 3x3
    let mut a = [
        [mat.get(0, 0), mat.get(0, 1), mat.get(0, 2)],
        [mat.get(1, 0), mat.get(1, 1), mat.get(1, 2)],
        [mat.get(2, 0), mat.get(2, 1), mat.get(2, 2)],
    ];
    // Eigenvectors start as identity
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    // Jacobi rotations
    for _ in 0..20 {
        // Find largest off-diagonal element
        let mut p = 0;
        let mut q = 1;
        let mut max_val = a[0][1].abs();
        for (pi, qi) in [(0, 2), (1, 2)] {
            if a[pi][qi].abs() > max_val {
                max_val = a[pi][qi].abs();
                p = pi;
                q = qi;
            }
        }

        if max_val < 1e-12 {
            break;
        }

        // Compute rotation angle
        let diff = a[q][q] - a[p][p];
        let t = if diff.abs() < 1e-15 {
            1.0
        } else {
            let phi = diff / (2.0 * a[p][q]);
            1.0 / (phi.abs() + (1.0 + phi * phi).sqrt())
                * if phi < 0.0 { -1.0 } else { 1.0 }
        };

        let c = 1.0 / (1.0 + t * t).sqrt();
        let s = t * c;

        // Apply Givens rotation to A: A' = G^T A G
        let app = a[p][p] - t * a[p][q];
        let aqq = a[q][q] + t * a[p][q];
        a[p][p] = app;
        a[q][q] = aqq;
        a[p][q] = 0.0;
        a[q][p] = 0.0;

        // Update remaining elements
        let r = 3 - p - q; // the third index
        let arp = a[r][p];
        let arq = a[r][q];
        a[r][p] = c * arp - s * arq;
        a[p][r] = a[r][p];
        a[r][q] = s * arp + c * arq;
        a[q][r] = a[r][q];

        // Update eigenvectors
        for i in 0..3 {
            let vip = v[i][p];
            let viq = v[i][q];
            v[i][p] = c * vip - s * viq;
            v[i][q] = s * vip + c * viq;
        }
    }

    let eigenvalues = [a[0][0], a[1][1], a[2][2]];
    // v[i][j] = row i, column j; columns are eigenvectors
    let eigenvectors = [
        [v[0][0], v[1][0], v[2][0]],
        [v[0][1], v[1][1], v[2][1]],
        [v[0][2], v[1][2], v[2][2]],
    ];
    (eigenvalues, eigenvectors)
}

pub struct QefSolver {
    ata: Sym3x3,
    atb: [f64; 3],
    btb: f64,
    mass_point_sum: [f64; 3],
    count: u32,
}

impl QefSolver {
    pub fn new() -> Self {
        Self {
            ata: Sym3x3::ZERO,
            atb: [0.0; 3],
            btb: 0.0,
            mass_point_sum: [0.0; 3],
            count: 0,
        }
    }

    pub fn add_plane(&mut self, point: [f32; 3], normal: [f32; 3]) {
        let n = [normal[0] as f64, normal[1] as f64, normal[2] as f64];
        let p = [point[0] as f64, point[1] as f64, point[2] as f64];

        self.ata.add_outer_product(n);
        let d = n[0] * p[0] + n[1] * p[1] + n[2] * p[2];
        self.atb[0] += n[0] * d;
        self.atb[1] += n[1] * d;
        self.atb[2] += n[2] * d;
        self.btb += d * d;

        self.mass_point_sum[0] += p[0];
        self.mass_point_sum[1] += p[1];
        self.mass_point_sum[2] += p[2];
        self.count += 1;
    }

    /// Solve the QEF, clamping result to [cell_min, cell_max].
    /// Returns (position, error).
    pub fn solve(&self, cell_min: [f32; 3], cell_max: [f32; 3]) -> ([f32; 3], f32) {
        if self.count == 0 {
            let center = [
                (cell_min[0] + cell_max[0]) * 0.5,
                (cell_min[1] + cell_max[1]) * 0.5,
                (cell_min[2] + cell_max[2]) * 0.5,
            ];
            return (center, 0.0);
        }

        let inv_count = 1.0 / self.count as f64;
        let mass_point = [
            self.mass_point_sum[0] * inv_count,
            self.mass_point_sum[1] * inv_count,
            self.mass_point_sum[2] * inv_count,
        ];

        // Solve relative to mass point for numerical stability
        // A^T A (v - mp) = A^T b - A^T A * mp
        let rhs = [
            self.atb[0]
                - (self.ata.m[0] * mass_point[0]
                + self.ata.m[1] * mass_point[1]
                + self.ata.m[2] * mass_point[2]),
            self.atb[1]
                - (self.ata.m[1] * mass_point[0]
                + self.ata.m[3] * mass_point[1]
                + self.ata.m[4] * mass_point[2]),
            self.atb[2]
                - (self.ata.m[2] * mass_point[0]
                + self.ata.m[4] * mass_point[1]
                + self.ata.m[5] * mass_point[2]),
        ];

        // Pseudoinverse via eigendecomposition
        let (eigenvalues, eigenvectors) = eigen_decompose(&self.ata);

        const THRESHOLD: f64 = 1e-6;
        let max_ev = eigenvalues[0]
            .abs()
            .max(eigenvalues[1].abs())
            .max(eigenvalues[2].abs());
        let threshold = max_ev * THRESHOLD;

        // v - mp = V * diag(1/sigma_i) * V^T * rhs
        let mut result = [0.0_f64; 3];
        for i in 0..3 {
            if eigenvalues[i].abs() > threshold {
                // Project rhs onto eigenvector i
                let dot = eigenvectors[i][0] * rhs[0]
                    + eigenvectors[i][1] * rhs[1]
                    + eigenvectors[i][2] * rhs[2];
                let scaled = dot / eigenvalues[i];
                result[0] += eigenvectors[i][0] * scaled;
                result[1] += eigenvectors[i][1] * scaled;
                result[2] += eigenvectors[i][2] * scaled;
            }
        }

        // Translate back from mass point
        let solved = [
            (result[0] + mass_point[0]) as f32,
            (result[1] + mass_point[1]) as f32,
            (result[2] + mass_point[2]) as f32,
        ];

        // Clamp to cell bounds
        let clamped = [
            solved[0].clamp(cell_min[0], cell_max[0]),
            solved[1].clamp(cell_min[1], cell_max[1]),
            solved[2].clamp(cell_min[2], cell_max[2]),
        ];

        // If clamping moved the vertex significantly, fall back to mass point
        let clamp_dist_sq = (clamped[0] - solved[0]).powi(2)
            + (clamped[1] - solved[1]).powi(2)
            + (clamped[2] - solved[2]).powi(2);

        // If the unclamped solution was far outside the cell, the QEF likely
        // had degenerate or contradictory constraints. Fall back to the mass
        // point (average of intersection points) which is always near the
        // isosurface. Threshold: one cell width squared.
        const CLAMP_FALLBACK_THRESHOLD: f32 = VOXEL_SCALE * VOXEL_SCALE;
        let final_pos = if clamp_dist_sq > CLAMP_FALLBACK_THRESHOLD {
            // Fall back to mass point, clamped
            [
                (mass_point[0] as f32).clamp(cell_min[0], cell_max[0]),
                (mass_point[1] as f32).clamp(cell_min[1], cell_max[1]),
                (mass_point[2] as f32).clamp(cell_min[2], cell_max[2]),
            ]
        } else {
            clamped
        };

        // Compute error (for diagnostics)
        let error = clamp_dist_sq.sqrt() as f32;

        (final_pos, error)
    }
}