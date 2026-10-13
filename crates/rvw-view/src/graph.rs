//! The lanes of the history graph, laid out one commit at a time so a page of
//! the log can be appended without redoing the rows above it.

/// A line drawn in one half of a row: from slot `from` at one edge to slot `to` at the other.
/// In `Row::top` the line runs from the row's top edge (slot `from`) to its middle (slot `to`).
/// In `Row::bottom` it runs from the middle (slot `from`) to the bottom edge (slot `to`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seg {
    pub from: usize,
    pub to: usize,
}

/// One commit's row of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The slot (column) of the commit's dot.
    pub lane: usize,
    /// Lines in the top half, sorted by (from, to).
    pub top: Vec<Seg>,
    /// Lines in the bottom half, sorted by (from, to).
    pub bottom: Vec<Seg>,
    /// Slots the row needs to draw: one more than the largest slot used in lane, top or bottom.
    pub width: usize,
}

/// The lanes so far; push commits newest first in topological order.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    /// The commit id each slot is waiting for; `None` is a free slot.
    slots: Vec<Option<String>>,
}

impl Graph {
    /// An empty graph with no lanes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lays out the next commit and returns its row. `parents` are in the
    /// order git lists them; the first parent continues this commit's lane.
    pub fn push(&mut self, id: &str, parents: &[String]) -> Row {
        let before = self.slots.clone();
        let lane = self.lane_for(id);

        // Lanes that were waiting for this commit end here.
        for (s, slot) in self.slots.iter_mut().enumerate() {
            if s != lane && slot.as_deref() == Some(id) {
                *slot = None;
            }
        }

        let mut top: Vec<Seg> = before
            .iter()
            .enumerate()
            .filter_map(|(s, slot)| {
                slot.as_ref().map(|waiting| Seg {
                    from: s,
                    to: if waiting == id { lane } else { s },
                })
            })
            .collect();
        top.sort_by_key(|seg| (seg.from, seg.to));

        self.slots[lane] = parents.first().cloned();
        let mut bottom = Vec::new();
        for parent in parents.iter().skip(1) {
            let waiting = self
                .slots
                .iter()
                .enumerate()
                .position(|(s, slot)| s != lane && slot.as_deref() == Some(parent.as_str()));
            let to = waiting.unwrap_or_else(|| {
                let s = self.free_slot();
                self.slots[s] = Some(parent.clone());
                s
            });
            bottom.push(Seg { from: lane, to });
        }

        // Lanes that run past this commit, and the dot's own lane if it goes on.
        for (s, slot) in self.slots.iter().enumerate() {
            let passes_through = s != lane
                && before
                    .get(s)
                    .and_then(Option::as_deref)
                    .is_some_and(|waiting| waiting != id);
            if passes_through && slot.is_some() {
                bottom.push(Seg { from: s, to: s });
            }
        }
        if self.slots[lane].is_some() {
            bottom.push(Seg {
                from: lane,
                to: lane,
            });
        }
        bottom.sort_by_key(|seg| (seg.from, seg.to));
        bottom.dedup();

        while matches!(self.slots.last(), Some(None)) {
            self.slots.pop();
        }

        let width = top
            .iter()
            .chain(&bottom)
            .flat_map(|seg| [seg.from, seg.to])
            .chain([lane])
            .max()
            .unwrap_or(0)
            + 1;

        Row {
            lane,
            top,
            bottom,
            width,
        }
    }

    /// The slot that waits for `id`, or else a free slot for a new branch tip.
    /// Either way the slot is returned, not yet filled in.
    fn lane_for(&mut self, id: &str) -> usize {
        let waiting = self
            .slots
            .iter()
            .position(|slot| slot.as_deref() == Some(id));
        waiting.unwrap_or_else(|| self.free_slot())
    }

    /// The first free slot, or a new slot at the end when none is free.
    fn free_slot(&mut self) -> usize {
        if let Some(s) = self.slots.iter().position(Option::is_none) {
            return s;
        }
        self.slots.push(None);
        self.slots.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(from: usize, to: usize) -> Seg {
        Seg { from, to }
    }

    #[test]
    fn linear_history_stays_in_lane_zero() {
        let mut g = Graph::new();

        assert_eq!(
            g.push("C", &["B".to_string()]),
            Row {
                lane: 0,
                top: vec![],
                bottom: vec![seg(0, 0)],
                width: 1,
            }
        );
        assert_eq!(
            g.push("B", &["A".to_string()]),
            Row {
                lane: 0,
                top: vec![seg(0, 0)],
                bottom: vec![seg(0, 0)],
                width: 1,
            }
        );
        assert_eq!(
            g.push("A", &[]),
            Row {
                lane: 0,
                top: vec![seg(0, 0)],
                bottom: vec![],
                width: 1,
            }
        );
    }

    #[test]
    fn merge_opens_a_second_lane_and_closes_it_into_the_parent() {
        let mut g = Graph::new();

        assert_eq!(
            g.push("M", &["A".to_string(), "B".to_string()]),
            Row {
                lane: 0,
                top: vec![],
                bottom: vec![seg(0, 0), seg(0, 1)],
                width: 2,
            }
        );
        assert_eq!(
            g.push("B", &["R".to_string()]),
            Row {
                lane: 1,
                top: vec![seg(0, 0), seg(1, 1)],
                bottom: vec![seg(0, 0), seg(1, 1)],
                width: 2,
            }
        );
        assert_eq!(
            g.push("A", &["R".to_string()]),
            Row {
                lane: 0,
                top: vec![seg(0, 0), seg(1, 1)],
                bottom: vec![seg(0, 0), seg(1, 1)],
                width: 2,
            }
        );
        assert_eq!(
            g.push("R", &[]),
            Row {
                lane: 0,
                top: vec![seg(0, 0), seg(1, 0)],
                bottom: vec![],
                width: 2,
            }
        );
    }

    #[test]
    fn two_tips_share_a_parent() {
        let mut g = Graph::new();

        assert_eq!(
            g.push("X", &["R".to_string()]),
            Row {
                lane: 0,
                top: vec![],
                bottom: vec![seg(0, 0)],
                width: 1,
            }
        );
        assert_eq!(
            g.push("Y", &["R".to_string()]),
            Row {
                lane: 1,
                top: vec![seg(0, 0)],
                bottom: vec![seg(0, 0), seg(1, 1)],
                width: 2,
            }
        );
        assert_eq!(
            g.push("R", &[]),
            Row {
                lane: 0,
                top: vec![seg(0, 0), seg(1, 0)],
                bottom: vec![],
                width: 2,
            }
        );
    }

    #[test]
    fn octopus_merge_opens_one_lane_per_parent() {
        let mut g = Graph::new();

        assert_eq!(
            g.push("O", &["A".to_string(), "B".to_string(), "C".to_string()]),
            Row {
                lane: 0,
                top: vec![],
                bottom: vec![seg(0, 0), seg(0, 1), seg(0, 2)],
                width: 3,
            }
        );
        assert_eq!(
            g.push("C", &[]),
            Row {
                lane: 2,
                top: vec![seg(0, 0), seg(1, 1), seg(2, 2)],
                bottom: vec![seg(0, 0), seg(1, 1)],
                width: 3,
            }
        );
        assert_eq!(
            g.push("B", &[]),
            Row {
                lane: 1,
                top: vec![seg(0, 0), seg(1, 1)],
                bottom: vec![seg(0, 0)],
                width: 2,
            }
        );
        assert_eq!(
            g.push("A", &[]),
            Row {
                lane: 0,
                top: vec![seg(0, 0)],
                bottom: vec![],
                width: 1,
            }
        );
    }

    #[test]
    fn freed_slot_is_reused_by_the_next_tip() {
        let mut g = Graph::new();

        g.push("T1", &["P".to_string()]);
        g.push("P", &[]);
        assert_eq!(
            g.push("T2", &["Q".to_string()]),
            Row {
                lane: 0,
                top: vec![],
                bottom: vec![seg(0, 0)],
                width: 1,
            }
        );
    }

    #[test]
    fn second_parent_already_waited_for_joins_that_lane() {
        let mut g = Graph::new();

        assert_eq!(
            g.push("N", &["B".to_string()]),
            Row {
                lane: 0,
                top: vec![],
                bottom: vec![seg(0, 0)],
                width: 1,
            }
        );
        assert_eq!(
            g.push("M", &["A".to_string(), "B".to_string()]),
            Row {
                lane: 1,
                top: vec![seg(0, 0)],
                bottom: vec![seg(0, 0), seg(1, 0), seg(1, 1)],
                width: 2,
            }
        );
        assert_eq!(
            g.push("A", &["B".to_string()]),
            Row {
                lane: 1,
                top: vec![seg(0, 0), seg(1, 1)],
                bottom: vec![seg(0, 0), seg(1, 1)],
                width: 2,
            }
        );
        assert_eq!(
            g.push("B", &[]),
            Row {
                lane: 0,
                top: vec![seg(0, 0), seg(1, 0)],
                bottom: vec![],
                width: 2,
            }
        );
    }

    #[test]
    fn paged_rows_match_one_pass_and_clones_continue_alike() {
        let commits: Vec<(&str, Vec<String>)> = vec![
            ("M", vec!["A".to_string(), "B".to_string()]),
            ("B", vec!["R".to_string()]),
            ("A", vec!["R".to_string()]),
            ("R", vec![]),
        ];

        let mut whole = Graph::new();
        let expected: Vec<Row> = commits
            .iter()
            .map(|(id, parents)| whole.push(id, parents))
            .collect();

        let mut first_page = Graph::new();
        let mut rows: Vec<Row> = commits[..2]
            .iter()
            .map(|(id, parents)| first_page.push(id, parents))
            .collect();

        let mut clone = first_page.clone();
        let mut rest_a: Vec<Row> = commits[2..]
            .iter()
            .map(|(id, parents)| first_page.push(id, parents))
            .collect();
        let mut rest_b: Vec<Row> = commits[2..]
            .iter()
            .map(|(id, parents)| clone.push(id, parents))
            .collect();

        rows.append(&mut rest_a);
        assert_eq!(rows, expected);
        assert_eq!(rest_b.len(), 2);
        assert_eq!(rest_b.remove(0), expected[2]);
        assert_eq!(rest_b.remove(0), expected[3]);
    }
}
