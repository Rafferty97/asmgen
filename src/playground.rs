use crate::bit_permutation::BitExtract;

/// Find the minimum-total-cost subset of `candidates` whose covers OR together
/// to cover every bit that *any* candidate covers. The set of needed bits is
/// taken to be the union (`universe`) of all candidate covers — bits that no
/// candidate ever supplies are treated as genuinely unwanted and ignored, so
/// they never force selection.
///
/// A solution always exists (the full candidate set trivially covers the
/// universe), so this returns a `Vec` directly rather than an `Option`.
///
/// Strategy: this is weighted set cover. We keep it exact but fast by:
///   1. Pruning dominated candidates (subset cover at >= cost is useless).
///   2. Forcing "essential" candidates (any bit covered by exactly one remaining
///      candidate must be selected).
///   3. Branch-and-bound on the least-covered uncovered bit, with an
///      admissible lower-bound to prune.
/// On the small, domination-heavy inputs this problem produces, the reductions
/// usually settle it before any real branching happens.
pub fn min_cost_cover(candidates: &[BitExtract]) -> Vec<usize> {
    // The bits we must cover are exactly those some candidate can supply.
    let universe: u64 = candidates.iter().fold(0, |acc, c| acc | c.cover);
    if universe == 0 {
        // No candidate covers anything: nothing needs covering, empty solution.
        return Vec::new();
    }

    // Work with (original_index, cost, cover). We never mutate candidates.
    let items: Vec<(usize, u8, u64)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.cover != 0) // zero-cover candidates can never help
        .map(|(i, c)| (i, c.cost, c.cover))
        .collect();

    let mut best: Option<(u32, Vec<usize>)> = None; // (total_cost, chosen indices)
    let mut chosen: Vec<usize> = Vec::new();

    solve(&items, universe, 0, &mut chosen, &mut best);

    // A solution is guaranteed to exist, so `best` is always Some here.
    best.expect("universe is coverable by the full candidate set")
        .1
}

/// Recursive branch-and-bound.
/// `items`: remaining usable candidates (index, cost, cover).
/// `remaining`: bits still needing coverage.
/// `acc_cost`: cost accumulated on the current path.
/// `chosen`: indices selected on the current path.
/// `best`: best complete solution found so far.
fn solve(
    items: &[(usize, u8, u64)],
    remaining: u64,
    acc_cost: u32,
    chosen: &mut Vec<usize>,
    best: &mut Option<(u32, Vec<usize>)>,
) {
    // Base case: everything covered.
    if remaining == 0 {
        match best {
            Some((bc, _)) if *bc <= acc_cost => {}
            _ => *best = Some((acc_cost, chosen.clone())),
        }
        return;
    }

    // Bound: if we've already matched or exceeded the best, abandon.
    if let Some((bc, _)) = best {
        if acc_cost >= *bc {
            return;
        }
    }

    // Restrict every candidate's cover to the bits that still matter.
    // Drop any that now cover nothing useful.
    let mut live: Vec<(usize, u8, u64)> = items
        .iter()
        .filter_map(|&(i, cost, cover)| {
            let c = cover & remaining;
            if c == 0 { None } else { Some((i, cost, c)) }
        })
        .collect();

    // Feasibility of this branch: the live candidates must still be able to
    // cover `remaining`. If not, dead end.
    let reachable: u64 = live.iter().fold(0, |a, &(_, _, c)| a | c);
    if reachable != remaining {
        return;
    }

    // --- Reduction 1: domination ---
    // If candidate A's (restricted) cover is a superset of B's and A costs <= B,
    // then B is useless on this branch: anything B could do, A does at least as
    // cheaply. Remove dominated candidates.
    // (O(n^2) but n is tiny here.)
    let mut keep = vec![true; live.len()];
    for a in 0..live.len() {
        if !keep[a] {
            continue;
        }
        for b in 0..live.len() {
            if a == b || !keep[b] {
                continue;
            }
            let (_, ca, cova) = live[a];
            let (_, cb, covb) = live[b];
            // A dominates B: A covers everything B does, at no greater cost,
            // and is not a strict-worse tie (avoid mutual elimination on exact
            // duplicates by only dropping the higher index on a true tie).
            let a_covers_b = (covb & !cova) == 0;
            if a_covers_b && ca <= cb {
                if ca == cb && cova == covb && a > b {
                    continue; // identical: keep the lower-indexed one only
                }
                keep[b] = false;
            }
        }
    }
    let mut idx = 0;
    live.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });

    // --- Reduction 2: essential candidates ---
    // Find a bit covered by exactly one live candidate. That candidate is forced.
    // Selecting all forced candidates first shrinks the problem with no branching.
    // We do one forced selection then recurse (which re-runs reductions).
    let mut forced: Option<usize> = None;
    // For each still-needed bit, count how many live candidates cover it.
    let mut r = remaining;
    while r != 0 {
        let bit = r & r.wrapping_neg(); // lowest set bit
        r &= r - 1;
        let mut count = 0u32;
        let mut who = 0usize;
        for (li, &(_, _, cover)) in live.iter().enumerate() {
            if cover & bit != 0 {
                count += 1;
                who = li;
                if count > 1 {
                    break;
                }
            }
        }
        if count == 1 {
            forced = Some(who);
            break;
        }
        // count == 0 is impossible here: reachable == remaining guaranteed it.
    }

    if let Some(li) = forced {
        let (orig, cost, cover) = live[li];
        chosen.push(orig);
        // Remove the forced candidate from the pool for the recursive call.
        let rest: Vec<(usize, u8, u64)> = live
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != li)
            .map(|(_, &t)| t)
            .collect();
        solve(
            &rest,
            remaining & !cover,
            acc_cost + cost as u32,
            chosen,
            best,
        );
        chosen.pop();
        return;
    }

    // --- Branch ---
    // Pick the "most constrained" uncovered bit: the still-needed bit covered by
    // the fewest live candidates. Branching on it keeps the tree narrow, since
    // one of that small set must be chosen.
    let mut target_bit = 0u64;
    let mut fewest = u32::MAX;
    let mut r = remaining;
    while r != 0 {
        let bit = r & r.wrapping_neg();
        r &= r - 1;
        let count = live
            .iter()
            .filter(|&&(_, _, cover)| cover & bit != 0)
            .count() as u32;
        if count < fewest {
            fewest = count;
            target_bit = bit;
        }
    }

    // Admissible lower bound for pruning: to cover `remaining`, we need at least
    // ceil(popcount(remaining) / max_cover_size) candidates, each costing at
    // least the minimum cost. This is cheap and never over-estimates.
    let min_cost = live.iter().map(|&(_, c, _)| c as u32).min().unwrap_or(0);
    let max_span = live
        .iter()
        .map(|&(_, _, cov)| cov.count_ones())
        .max()
        .unwrap_or(1)
        .max(1);
    let need = remaining.count_ones();
    let min_candidates = need.div_ceil(max_span);
    let lower_bound = acc_cost + min_candidates * min_cost;
    if let Some((bc, _)) = best {
        if lower_bound >= *bc {
            return;
        }
    }

    // Try each candidate covering the target bit, most-covering first (helps find
    // a good incumbent early, which strengthens the bound).
    let mut branchers: Vec<(usize, u8, u64)> = live
        .iter()
        .copied()
        .filter(|&(_, _, cover)| cover & target_bit != 0)
        .collect();
    branchers.sort_by_key(|&(_, cost, cover)| (u32::MAX - cover.count_ones(), cost));

    for (orig, cost, cover) in branchers {
        chosen.push(orig);
        // Remove just this candidate; others remain available.
        let rest: Vec<(usize, u8, u64)> = live
            .iter()
            .copied()
            .filter(|&(i, _, _)| i != orig)
            .collect();
        solve(
            &rest,
            remaining & !cover,
            acc_cost + cost as u32,
            chosen,
            best,
        );
        chosen.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost_of(cands: &[BitExtract], pick: &[usize]) -> u32 {
        pick.iter().map(|&i| cands[i].cost as u32).sum()
    }
    fn universe(cands: &[BitExtract]) -> u64 {
        cands.iter().fold(0u64, |a, c| a | c.cover)
    }
    fn covers_all(cands: &[BitExtract], pick: &[usize]) -> bool {
        pick.iter().fold(0u64, |a, &i| a | cands[i].cover) == universe(cands)
    }

    /// Brute-force reference: try every subset, return the min cost that covers
    /// the universe (the OR of all candidate covers). Always Some, since the
    /// full set covers the universe by definition.
    fn brute(cands: &[BitExtract]) -> u32 {
        let n = cands.len();
        let uni = universe(cands);
        let mut best = None;
        for mask in 0u64..(1u64 << n) {
            let mut cover = 0u64;
            let mut cost = 0u32;
            for i in 0..n {
                if mask & (1 << i) != 0 {
                    cover |= cands[i].cover;
                    cost += cands[i].cost as u32;
                }
            }
            if cover == uni {
                best = Some(best.map_or(cost, |b: u32| b.min(cost)));
            }
        }
        best.unwrap()
    }

    #[test]
    fn dont_care_bits_are_ignored() {
        // No candidate covers bit 0, so bit 0 is not part of the universe and
        // must not force anything. The single candidate is the whole solution.
        let cands = vec![BitExtract { cost: 1, cover: !0 << 1, ..Default::default() }];
        let pick = min_cost_cover(&cands);
        assert!(covers_all(&cands, &pick)); // covers the universe (everything but bit 0)
        assert_eq!(cost_of(&cands, &pick), 1);
    }

    #[test]
    fn empty_input_is_empty_solution() {
        let cands: Vec<BitExtract> = vec![];
        assert!(min_cost_cover(&cands).is_empty());
    }

    #[test]
    fn all_zero_cover_is_empty_solution() {
        // Universe is empty; nothing needs covering.
        let cands = vec![BitExtract { cost: 3, cover: 0, ..Default::default() }];
        assert!(min_cost_cover(&cands).is_empty());
    }

    #[test]
    fn single_full_cover() {
        let cands = vec![BitExtract { cost: 5, cover: !0, ..Default::default() }];
        let pick = min_cost_cover(&cands);
        assert!(covers_all(&cands, &pick));
        assert_eq!(cost_of(&cands, &pick), 5);
    }

    #[test]
    fn prefers_cheaper_full_cover() {
        let cands = vec![
            BitExtract { cost: 5, cover: !0, ..Default::default() },
            BitExtract { cost: 3, cover: !0, ..Default::default() },
            BitExtract { cost: 9, cover: !0, ..Default::default() },
        ];
        let pick = min_cost_cover(&cands);
        assert_eq!(cost_of(&cands, &pick), 3);
    }

    #[test]
    fn two_halves() {
        let lo = 0x0000_0000_FFFF_FFFF;
        let hi = 0xFFFF_FFFF_0000_0000;
        let cands = vec![
            BitExtract { cost: 2, cover: lo, ..Default::default() },
            BitExtract { cost: 2, cover: hi, ..Default::default() },
            BitExtract { cost: 10, cover: !0, ..Default::default() }, // one-shot but pricey
        ];
        let pick = min_cost_cover(&cands);
        assert!(covers_all(&cands, &pick));
        assert_eq!(cost_of(&cands, &pick), 4); // two halves beat the 10 one-shot
    }

    #[test]
    fn essential_forced() {
        // Only one candidate covers bit 63; it must be chosen.
        let cands = vec![
            BitExtract { cost: 1, cover: 1 << 63, ..Default::default() },
            BitExtract { cost: 1, cover: (!0) >> 1, ..Default::default() }, // everything except bit 63
        ];
        let pick = min_cost_cover(&cands);
        assert!(covers_all(&cands, &pick));
        assert_eq!(cost_of(&cands, &pick), 2);
    }

    #[test]
    fn random_matches_bruteforce() {
        // Fuzz against brute force on small instances. Uses narrow covers too,
        // so many instances have don't-care bits (universe != !0).
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut rng = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..4000 {
            let n = (rng() % 8) as usize + 1;
            let mut cands = Vec::new();
            for _ in 0..n {
                cands.push(BitExtract {
                    cost: (rng() % 8) as u8 + 1,
                    // Mix wide and narrow covers; narrow ones leave don't-care bits.
                    cover: if rng() & 1 == 0 {
                        rng() | rng()
                    } else {
                        rng() & rng()
                    },
                    ..Default::default()
                });
            }
            let pick = min_cost_cover(&cands);
            assert!(
                covers_all(&cands, &pick),
                "returned pick doesn't cover the universe: {:?}",
                cands
            );
            let got = cost_of(&cands, &pick);
            let want = brute(&cands);
            assert_eq!(got, want, "mismatch on {:?}", cands);
        }
    }
}
