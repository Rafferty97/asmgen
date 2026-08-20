use itertools::Itertools;

use crate::{bit_permutation::BitExtract, bit_utils::iter_set_bits};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Candidate {
    sh_mask: u64,
    bc_mask: u64,
    shbc_mask: u64,
    cost: u16,
}

// #[derive(Clone, Copy, PartialEq, Eq, Debug)]
// struct Candidate {
//     shift_mask: u64,
//     /// Maps broadcast -> shift to fuse, if any
//     /// N @ 0-63 = fuse with shift N
//     /// 254 = bare broadcast
//     /// 255 = no broadcast
//     broadcasts: [u8; 64],
//     cost: u16,
// }

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.cost.cmp(&other.cost).reverse()
    }
}

// impl Default for Candidate {
//     fn default() -> Self {
//         Candidate { shift_mask: u64::MAX, broadcasts: [0; 64], cost: u16::MAX }
//     }
// }

fn covers(a: u64, b: u64) -> bool {
    a | b == a
}

/// Cost-to-cover ratio. Lower is better.
#[derive(Clone, Copy, Debug)]
struct CostCover {
    cost: u16,
    cover: u16,
}

impl Ord for CostCover {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.cost * other.cover).cmp(&(other.cost * self.cover))
    }
}

impl PartialOrd for CostCover {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for CostCover {}

impl PartialEq for CostCover {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

pub fn min_cost_cover2(
    shifts: &[BitExtract],
    broadcasts: &[BitExtract],
    repeats: &[BitExtract],
    shift_broadcasts: &[BitExtract],
) -> Vec<BitExtract> {
    let start = std::time::Instant::now();

    // Merge the candidate pool
    let mut candidates = shifts
        .iter()
        .chain(broadcasts)
        .chain(repeats)
        .chain(shift_broadcasts)
        .collect_vec();

    // The bits we must cover are exactly those some candidate can supply.
    let universe: u64 = candidates.iter().fold(0, |acc, c| acc | c.dst_bits());
    if universe == 0 {
        // No candidate covers anything: nothing needs covering, empty solution.
        return Vec::new();
    }

    // Initial state
    let mut cover = 0;
    let mut chosen = Vec::<&BitExtract>::new();

    while cover != universe {
        // Filter out dead candidates
        candidates.retain(|c| !covers(cover, c.dst_bits()));

        // Score the remaining candidates
        let candidate_costs = candidates.iter().map(|c| {
            let cover = (c.dst_bits() & !cover).count_ones() as u16;
            let added_cost = c.cost();
            let saved_cost = chosen
                .iter()
                .filter(|d| covers(c.dst_bits(), d.dst_bits()))
                .map(|d| d.cost())
                .sum::<u16>();
            let cost = added_cost.saturating_sub(saved_cost);
            (c, CostCover { cost, cover })
        });

        // Pick the best
        let (&best, _) = candidate_costs.min_by_key(|&(_, cost)| cost).unwrap();

        // Add it to the set and remove subsumed candidates
        chosen.retain(|c| !covers(best.dst_bits(), c.dst_bits()));
        chosen.push(best);

        // Update the new cover
        cover |= best.dst_bits();
    }

    // println!("min_cost_cover took {:?}", start.elapsed());

    return chosen.into_iter().cloned().collect();

    // let mut complete = 0;
    // let mut partial = 0;
    // for shbc in shift_broadcasts {
    //     let shift = shifts
    //         .iter()
    //         .find(|sh| sh.net_ror() == shbc.net_ror())
    //         .unwrap();
    //     let broadcast = broadcasts
    //         .iter()
    //         .find(|bc| covers(shbc.dst_bits(), bc.dst_bits()))
    //         .unwrap();
    //     if covers(shbc.dst_bits(), shift.dst_bits() | broadcast.dst_bits()) {
    //         complete += 1;
    //     } else {
    //         partial += 1;
    //     }
    // }
    // dbg!(complete);
    // dbg!(partial);

    // let mut best = vec![0; 1 << broadcasts.len()];
    // let mut prev = vec![0; 1 << broadcasts.len()];

    // for (shift_idx, shift) in shifts.iter().enumerate() {
    //     prev.copy_from_slice(&best);

    //     let net_ror = shift.net_ror();
    //     let shift_broadcasts = shift_broadcasts
    //         .iter()
    //         .enumerate()
    //         .filter(|(_, shbc)| shbc.net_ror() == net_ror)
    //         .collect_vec();

    //     for bc_mask in 0..best.len() {
    //         for &(shbc_idx, shbc) in &shift_broadcasts {
    //             let (bc_idx, bc) = broadcasts
    //                 .iter()
    //                 .enumerate()
    //                 .find(|(_, bc)| bc.dst_bits() & !shbc.dst_bits() == 0)
    //                 .unwrap();
    //             if (bc_mask >> bc_idx) & 1 != 0 {
    //                 // already claimed
    //                 continue;
    //             }

    //             let new_bc_mask = bc_mask | (1 << bc_idx);
    //             let cost = prev[bc_mask] + shbc.cost() - bc.cost();
    //             best[new_bc_mask] = best[new_bc_mask].min(cost);
    //         }
    //     }
    // }

    // // Compute the covered bits for every combination of broadcasts
    // let mut bc_covers = vec![0; 1 << broadcasts.len()];
    // for (idx, broadcast) in broadcasts.iter().enumerate() {
    //     let cover = broadcast.dst_bits();
    //     let mut bc_mask = 1 << idx;
    //     while bc_mask < bc_covers.len() {
    //         bc_covers[bc_mask] |= cover;
    //         bc_mask = (bc_mask + 1) | (1 << idx);
    //     }
    // }

    // let bc_rows = bc_covers
    //     .into_iter()
    //     .map(|cover| {
    //         let mut out = 0u64;
    //         for (idx, shift) in shifts.iter().enumerate() {
    //             if shift.dst_bits() & !cover == 0 {
    //                 out |= 1 << idx;
    //             }
    //         }
    //         out
    //     })
    //     .collect_vec();

    // // For all subsets of broadcasts
    // let mut minimal_masks = vec![];
    // for (bc_mask, &cover) in bc_rows.iter().enumerate() {
    //     // Check that no broadcast is redundant
    //     let dominated = iter_set_bits(bc_mask as u64)
    //         .map(|bit| bc_mask & !(1 << bit))
    //         .any(|bc_mask2| cover == bc_rows[bc_mask2]);
    //     if !dominated {
    //         minimal_masks.push(bc_mask);
    //     }
    // }

    // println!("Number of minimal masks = {}", minimal_masks.len());

    // let mut best = Candidate { sh_mask: 0, bc_mask: 0, shbc_mask: 0, cost: u16::MAX };

    // for bc_mask in 0..(1 << broadcasts.len()) {
    //     for shbc_mask in 0..(1 << broadcasts.len()) {
    //         let fixed_cover = select_subset(broadcasts, bc_mask)
    //             .chain(select_subset(shift_broadcasts, shbc_mask))
    //             .fold(0, |acc, ex| acc | ex.dst_bits());

    //         let fixed_cost = select_subset(broadcasts, bc_mask)
    //             .chain(select_subset(shift_broadcasts, shbc_mask))
    //             .fold(0, |acc, ex| acc + ex.cost());

    //         if fixed_cost > best.cost {
    //             continue;
    //         }

    //         let (sh_mask, shift_cost) = select_uncovered(shifts, !fixed_cover);

    //         let cost = fixed_cost + shift_cost;
    //         let candidate = Candidate { sh_mask, bc_mask, shbc_mask, cost };
    //         best = best.max(candidate);
    //     }
    // }

    // println!("best candidate = {:?}", best);

    // let result = select_subset(shifts, best.sh_mask)
    //     .chain(select_subset(broadcasts, best.bc_mask))
    //     .chain(select_subset(shift_broadcasts, best.shbc_mask))
    //     .cloned()
    //     .collect_vec();

    // let expected_cover = shifts.iter().fold(0, |acc, ex| acc | ex.dst_bits());
    // let actual_cover = result.iter().fold(0, |acc, ex| acc | ex.dst_bits());
    // debug_assert_eq!(expected_cover, actual_cover);

    // result
}

fn select_subset<T>(items: &[T], mask: u64) -> impl Iterator<Item = &T> {
    items
        .iter()
        .enumerate()
        .take(63)
        .filter(move |(idx, _)| (mask >> idx) & 1 != 0)
        .map(|(_, item)| item)
}

fn select_uncovered(items: &[BitExtract], uncovered: u64) -> (u64, u16) {
    let mut mask = 0;
    let mut cost = 0;
    for (idx, item) in items.iter().enumerate() {
        if item.dst_bits() & uncovered == 0 {
            continue;
        }
        mask |= 1 << idx;
        cost += item.cost();
    }
    (mask, cost)
}

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
    let start = std::time::Instant::now();

    // The bits we must cover are exactly those some candidate can supply.
    let universe: u64 = candidates.iter().fold(0, |acc, c| acc | c.dst_bits());
    if universe == 0 {
        // No candidate covers anything: nothing needs covering, empty solution.
        return Vec::new();
    }

    // Work with (original_index, cost, cover). We never mutate candidates.
    let items: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.dst_bits() != 0) // zero-cover candidates can never help
        .map(|(i, c)| (i, c.cost(), c.dst_bits()))
        .collect();

    let mut best: Option<(u32, Vec<usize>)> = None; // (total_cost, chosen indices)
    let mut chosen: Vec<usize> = Vec::new();

    solve(&items, universe, 0, &mut chosen, &mut best);

    // println!("min_cost_cover took {:?}", start.elapsed());

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
    items: &[(usize, u16, u64)],
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
    let mut live: Vec<_> = items
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
        let rest: Vec<_> = live
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
    let mut branchers: Vec<_> = live
        .iter()
        .copied()
        .filter(|&(_, _, cover)| cover & target_bit != 0)
        .collect();
    branchers.sort_by_key(|&(_, cost, cover)| (u32::MAX - cover.count_ones(), cost));

    for (orig, cost, cover) in branchers {
        chosen.push(orig);
        // Remove just this candidate; others remain available.
        let rest: Vec<_> = live
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

/// Remove candidates that can never appear in an optimal cover.
///
/// Candidate A dominates candidate B when A's cover is a superset of B's and A
/// costs no more: any solution using B can substitute A without covering less
/// or paying more, so B is redundant. Dropping dominated candidates is exact —
/// it cannot change the optimal cost.
///
/// This is the *global* form of the domination check `solve` already performs
/// per-branch against the restricted covers. Running it once up front doesn't
/// prune anything the recursion wouldn't, but it shrinks the pool that every
/// node re-scans, which matters when the candidate generator emits a large
/// near-cartesian product.
///
/// Candidates with an empty cover are dropped: they contribute nothing and
/// would otherwise be dominated by everything.
///
/// Returns the surviving candidates in their original relative order.
pub fn prune_dominated_candidates(candidates: &[BitExtract]) -> Vec<BitExtract> {
    let items: Vec<(u16, u64)> = candidates
        .iter()
        .map(|c| (c.cost(), c.dst_bits()))
        .collect();

    let n = items.len();
    let mut keep = vec![true; n];

    for a in 0..n {
        let (cost_a, cover_a) = items[a];

        // An empty cover can never help; drop it outright.
        if cover_a == 0 {
            keep[a] = false;
            continue;
        }

        if !keep[a] {
            continue;
        }

        for b in 0..n {
            if a == b || !keep[b] {
                continue;
            }

            let (cost_b, cover_b) = items[b];

            // A dominates B iff A covers everything B covers, at no greater cost.
            let a_covers_b = (cover_b & !cover_a) == 0;
            if !a_covers_b || cost_a > cost_b {
                continue;
            }

            // Exact duplicates dominate each other; keep only the lower index so
            // they don't mutually eliminate.
            if cost_a == cost_b && cover_a == cover_b && a > b {
                continue;
            }

            keep[b] = false;
        }
    }

    candidates
        .iter()
        .zip(keep)
        .filter_map(|(c, k)| k.then(|| c.clone()))
        .collect()
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     fn cost_of(cands: &[BitExtract], pick: &[usize]) -> u32 {
//         pick.iter().map(|&i| cands[i].cost as u32).sum()
//     }
//     fn universe(cands: &[BitExtract]) -> u64 {
//         cands.iter().fold(0u64, |a, c| a | c.dst_bits)
//     }
//     fn covers_all(cands: &[BitExtract], pick: &[usize]) -> bool {
//         pick.iter().fold(0u64, |a, &i| a | cands[i].dst_bits) == universe(cands)
//     }

//     /// Brute-force reference: try every subset, return the min cost that covers
//     /// the universe (the OR of all candidate covers). Always Some, since the
//     /// full set covers the universe by definition.
//     fn brute(cands: &[BitExtract]) -> u32 {
//         let n = cands.len();
//         let uni = universe(cands);
//         let mut best = None;
//         for mask in 0u64..(1u64 << n) {
//             let mut cover = 0u64;
//             let mut cost = 0u32;
//             for i in 0..n {
//                 if mask & (1 << i) != 0 {
//                     cover |= cands[i].dst_bits;
//                     cost += cands[i].cost as u32;
//                 }
//             }
//             if cover == uni {
//                 best = Some(best.map_or(cost, |b: u32| b.min(cost)));
//             }
//         }
//         best.unwrap()
//     }

//     #[test]
//     fn dont_care_bits_are_ignored() {
//         // No candidate covers bit 0, so bit 0 is not part of the universe and
//         // must not force anything. The single candidate is the whole solution.
//         let cands = vec![BitExtract { cost: 1, dst_bits: !0 << 1, ..Default::default() }];
//         let pick = min_cost_cover(&cands);
//         assert!(covers_all(&cands, &pick)); // covers the universe (everything but bit 0)
//         assert_eq!(cost_of(&cands, &pick), 1);
//     }

//     #[test]
//     fn empty_input_is_empty_solution() {
//         let cands: Vec<BitExtract> = vec![];
//         assert!(min_cost_cover(&cands).is_empty());
//     }

//     #[test]
//     fn all_zero_cover_is_empty_solution() {
//         // Universe is empty; nothing needs covering.
//         let cands = vec![BitExtract { cost: 3, dst_bits: 0, ..Default::default() }];
//         assert!(min_cost_cover(&cands).is_empty());
//     }

//     #[test]
//     fn single_full_cover() {
//         let cands = vec![BitExtract { cost: 5, dst_bits: !0, ..Default::default() }];
//         let pick = min_cost_cover(&cands);
//         assert!(covers_all(&cands, &pick));
//         assert_eq!(cost_of(&cands, &pick), 5);
//     }

//     #[test]
//     fn prefers_cheaper_full_cover() {
//         let cands = vec![
//             BitExtract { cost: 5, dst_bits: !0, ..Default::default() },
//             BitExtract { cost: 3, dst_bits: !0, ..Default::default() },
//             BitExtract { cost: 9, dst_bits: !0, ..Default::default() },
//         ];
//         let pick = min_cost_cover(&cands);
//         assert_eq!(cost_of(&cands, &pick), 3);
//     }

//     #[test]
//     fn two_halves() {
//         let lo = 0x0000_0000_FFFF_FFFF;
//         let hi = 0xFFFF_FFFF_0000_0000;
//         let cands = vec![
//             BitExtract { cost: 2, dst_bits: lo, ..Default::default() },
//             BitExtract { cost: 2, dst_bits: hi, ..Default::default() },
//             BitExtract { cost: 10, dst_bits: !0, ..Default::default() }, // one-shot but pricey
//         ];
//         let pick = min_cost_cover(&cands);
//         assert!(covers_all(&cands, &pick));
//         assert_eq!(cost_of(&cands, &pick), 4); // two halves beat the 10 one-shot
//     }

//     #[test]
//     fn essential_forced() {
//         // Only one candidate covers bit 63; it must be chosen.
//         let cands = vec![
//             BitExtract { cost: 1, dst_bits: 1 << 63, ..Default::default() },
//             BitExtract { cost: 1, dst_bits: (!0) >> 1, ..Default::default() }, // everything except bit 63
//         ];
//         let pick = min_cost_cover(&cands);
//         assert!(covers_all(&cands, &pick));
//         assert_eq!(cost_of(&cands, &pick), 2);
//     }

//     #[test]
//     fn random_matches_bruteforce() {
//         // Fuzz against brute force on small instances. Uses narrow covers too,
//         // so many instances have don't-care bits (universe != !0).
//         let mut state = 0x1234_5678_9abc_def0u64;
//         let mut rng = || {
//             state ^= state << 13;
//             state ^= state >> 7;
//             state ^= state << 17;
//             state
//         };
//         for _ in 0..4000 {
//             let n = (rng() % 8) as usize + 1;
//             let mut cands = Vec::new();
//             for _ in 0..n {
//                 cands.push(BitExtract {
//                     cost: (rng() % 8) as u8 + 1,
//                     // Mix wide and narrow covers; narrow ones leave don't-care bits.
//                     dst_bits: if rng() & 1 == 0 {
//                         rng() | rng()
//                     } else {
//                         rng() & rng()
//                     },
//                     ..Default::default()
//                 });
//             }
//             let pick = min_cost_cover(&cands);
//             assert!(
//                 covers_all(&cands, &pick),
//                 "returned pick doesn't cover the universe: {:?}",
//                 cands
//             );
//             let got = cost_of(&cands, &pick);
//             let want = brute(&cands);
//             assert_eq!(got, want, "mismatch on {:?}", cands);
//         }
//     }
// }
