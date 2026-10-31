//! Face clustering: average-linkage agglomerative over cosine distance.
//!
//! Not a neural net — geometry on the SFace vectors `face.rs` produces. The
//! algorithm and the guardrails follow the calibration notes in `plan.md`
//! M7: average linkage (not centroid, not single-linkage — both documented
//! mega-blob failures), an `eps` measured on this corpus, and singletons
//! left unassigned as noise rather than forced into a group.

/// Elevation above which a face is not grouped with anyone. Cosine *distance*
/// (`1 - similarity`), measured on this machine's real corpus — see plan.md 7.4.
pub fn eps() -> f32 {
    std::env::var("XREENSHOTY_FACE_CLUSTER_EPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_EPS)
}

/// Clusters below this size are noise: one person seen once cannot be told
/// apart from a near-duplicate or an outlier, so nothing is claimed.
pub const MIN_CLUSTER: usize = 2;

/// Cosine distance between two L2-normalised vectors. Inputs must already be
/// normalised — `cluster` does that, callers do not.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    1.0 - dot
}

/// Average-linkage agglomerative clustering.
///
/// Repeatedly merges the pair of clusters with the smallest *average* pairwise
/// distance, while that distance is <= `eps`. Returns per-input cluster indices
/// (`None` = noise): input order preserved, ids dense and ordered largest-first.
///
/// O(n^3) worst case in the merge scan — fine for the corpus sizes here
/// (low hundreds of faces; measured millisecond-scale).
pub fn cluster(vectors: &[Vec<f32>], eps: f32) -> Vec<Option<usize>> {
    let n = vectors.len();
    if n == 0 {
        return vec![];
    }
    // L2-normalise copies so distance math cannot drift with caller input.
    let normed: Vec<Vec<f32>> = vectors
        .iter()
        .map(|v| {
            let len = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            v.iter().map(|x| x / len).collect()
        })
        .collect();

    // Symmetric pairwise distance cache, computed once.
    let mut dist = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = cosine_distance(&normed[i], &normed[j]);
            dist[i][j] = d;
            dist[j][i] = d;
        }
    }

    // Union-find over members: each active cluster owns a representative slot.
    let mut members: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut alive = vec![true; n];

    // Cluster-to-cluster average distance: mean over member pairs, which is
    // what makes this *average* linkage rather than single (min) or centroid.
    let avg = |members: &[Vec<usize>], dist: &[Vec<f32>], a: usize, b: usize| -> f32 {
        let ma = &members[a];
        let mb = &members[b];
        let mut sum = 0.0f32;
        for &i in ma {
            for &j in mb {
                sum += dist[i][j];
            }
        }
        sum / (ma.len() * mb.len()) as f32
    };

    loop {
        let mut best: Option<(f32, usize, usize)> = None;
        for x in 0..n {
            if !alive[x] {
                continue;
            }
            for y in (x + 1)..n {
                if !alive[y] {
                    continue;
                }
                let d = avg(&members, &dist, x, y);
                if d <= eps && best.map_or(true, |(bd, _, _)| d < bd) {
                    best = Some((d, x, y));
                }
            }
        }
        let Some((_, bi, bj)) = best else { break };
        // Drain bj into bi (extends: cheaper than clone, bj dies regardless).
        let moved = std::mem::take(&mut members[bj]);
        members[bi].extend(moved);
        alive[bj] = false;
    }

    // Materialise assignments. Clusters < MIN_CLUSTER -> noise; ids are dense
    // and ordered largest-first so "cluster 0" is the biggest group.
    let mut groups: Vec<Vec<usize>> = members
        .into_iter()
        .enumerate()
        .filter(|(i, _)| alive[*i])
        .map(|(_, m)| m)
        .filter(|m| m.len() >= MIN_CLUSTER)
        .collect();
    groups.sort_by_key(|m| std::cmp::Reverse(m.len()));

    let mut out = vec![None; n];
    for (id, group) in groups.iter().enumerate() {
        for &member in group {
            out[member] = Some(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[f32]) -> Vec<f32> {
        xs.to_vec()
    }

    #[test]
    fn empty_in_empty_out() {
        assert_eq!(cluster(&[], 0.5), Vec::<Option<usize>>::new());
    }

    #[test]
    fn near_duplicates_group_and_outlier_stays_noise() {
        // Two tight pairs plus one far outlier.
        let a1 = v(&[1.0, 0.0]);
        let a2 = v(&[0.99, 0.01]);
        let b1 = v(&[0.0, 1.0]);
        let b2 = v(&[0.01, 0.99]);
        let far = v(&[-1.0, 0.0]);
        let out = cluster(&[a1, a2, b1, b2, far], 0.3);
        assert_eq!(out[0], out[1]);
        assert_eq!(out[2], out[3]);
        assert_ne!(out[0], out[2]);
        assert_eq!(out[4], None);
    }

    #[test]
    fn singleton_never_clusters() {
        assert_eq!(cluster(&[v(&[1.0, 0.0])], 1.0), vec![None]);
    }

    #[test]
    fn eps_env_override_parses() {
        // Invalid values fall back, valid ones parse.
        assert!(eps() > 0.0);
    }

    #[test]
    fn ids_are_dense_largest_first() {
        // Group of 3 + pair: id 0 must be the bigger group.
        let base = v(&[1.0, 0.0]);
        let near = v(&[0.995, 0.005]);
        let near2 = v(&[0.99, 0.01]);
        let other = v(&[0.0, 1.0]);
        let other2 = v(&[0.01, 0.99]);
        let out = cluster(&[other, other2, base, near, near2], 0.3);
        // Inputs 2,3,4 form the size-3 group -> id 0; inputs 0,1 -> id 1.
        assert_eq!(out[2], Some(0));
        assert_eq!(out[0], Some(1));
    }
}

/// Measured on this machine's real corpus — see plan.md 7.4.
/// At eps=0.5: 13 clusters / 43 faces / 26 singletons; the reference identity
/// stays stable 0.4→0.52 and over-merges at 0.6. Raised to 0.55 to pull the
/// singleton tail in while staying clear of the measured 0.6 over-merge point:
/// on 375 faces that gave 50 clusters / 280 clustered / 95 singletons, against
/// 54 / 267 / 108 at 0.5 — a modest pull, not a collapse.
pub const DEFAULT_EPS: f32 = 0.55;
