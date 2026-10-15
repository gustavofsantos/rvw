//! The sidebar model: projects, lanes, their order and dot state (M7 slice 4).
//! Pure: the caller reads the project files, the worktrees and the facts, and
//! hands over plain inputs. Order is the project file's, never urgency's; a
//! lane stacked `after` another sits directly under it, indented.

use std::path::PathBuf;

use rvw_project::Project;

/// One lane as the caller found it: a lane of the project file, or a worktree
/// the file does not name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneInput {
    pub name: String,
    /// The branch checked out, when there is one.
    pub branch: Option<String>,
    /// The worktree's directory; `None` when the worktree is missing.
    pub path: Option<PathBuf>,
    /// The repository's main checkout.
    pub main: bool,
    /// The lane waits for the reviewer (the orange dot), computed elsewhere.
    pub waiting: bool,
    /// The lane has a command or terminal running.
    pub running: bool,
}

/// One project as the caller found it.
#[derive(Debug, Clone)]
pub struct ProjectInput<'a> {
    pub project: &'a Project,
    /// The project file's lanes first, in file order, then the discovered
    /// worktrees the file does not name.
    pub lanes: Vec<LaneInput>,
    /// The project is expanded in the sidebar.
    pub open: bool,
}

/// The sidebar's projects, in the order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidebar {
    pub projects: Vec<ProjectGroup>,
}

/// One project's group in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectGroup {
    pub name: String,
    /// Expanded. A collapsed project lists no lanes.
    pub open: bool,
    /// Lanes waiting for the reviewer, counted even when the group is collapsed.
    pub waiting_lanes: usize,
    pub lanes: Vec<LaneRow>,
}

/// One lane row in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneRow {
    pub name: String,
    pub branch: Option<String>,
    /// The `↳` indent: 1 + the depth of the lane it is stacked `after`; 0 when
    /// it stacks on none, or on a lane the sidebar does not list.
    pub depth: usize,
    pub main: bool,
    /// The worktree is gone.
    pub missing: bool,
    pub dot: Dot,
}

/// A lane's dot. Waiting wins over running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    Idle,
    Running,
    Waiting,
}

/// The sidebar for the projects, in the order given.
#[must_use]
pub fn build(projects: &[ProjectInput<'_>]) -> Sidebar {
    Sidebar {
        projects: projects.iter().map(group).collect(),
    }
}

/// The lane `delta` steps from `current` (a project name and a lane name) in
/// the visible lanes: the lanes of the expanded projects, in sidebar order,
/// wrapping at both ends. With no visible current lane, a positive delta
/// selects the first and a negative one the last; zero selects nothing.
/// `None` when no lane is visible.
#[must_use]
pub fn step(
    sidebar: &Sidebar,
    current: Option<(&str, &str)>,
    delta: isize,
) -> Option<(String, String)> {
    let visible: Vec<(&str, &str)> = sidebar
        .projects
        .iter()
        .flat_map(|group| {
            group
                .lanes
                .iter()
                .map(move |lane| (group.name.as_str(), lane.name.as_str()))
        })
        .collect();
    let len = isize::try_from(visible.len()).ok()?;
    if len == 0 {
        return None;
    }
    let steps = delta.rem_euclid(len);
    let index = match current.and_then(|c| visible.iter().position(|v| *v == c)) {
        Some(index) => isize::try_from(index).ok()?,
        None if delta > 0 => -1,
        None if delta < 0 => len,
        None => return None,
    };
    let (project, lane) = visible[usize::try_from((index + steps).rem_euclid(len)).ok()?];
    Some((project.to_owned(), lane.to_owned()))
}

fn group(input: &ProjectInput<'_>) -> ProjectGroup {
    let waiting_lanes = input.lanes.iter().filter(|lane| lane.waiting).count();
    let lanes = if input.open { rows(input) } else { Vec::new() };
    ProjectGroup {
        name: input.project.name.clone(),
        open: input.open,
        waiting_lanes,
        lanes,
    }
}

/// The project's rows: each root lane in file order, each followed by its
/// stacked lanes, depth first, in file order among siblings.
fn rows(input: &ProjectInput<'_>) -> Vec<LaneRow> {
    let parents = parents(input);
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); parents.len()];
    let mut roots = Vec::new();
    for (index, parent) in parents.iter().enumerate() {
        match parent {
            Some(parent) => children[*parent].push(index),
            None => roots.push(index),
        }
    }
    let mut out = Vec::with_capacity(parents.len());
    let mut stack: Vec<(usize, usize)> = roots.into_iter().rev().map(|index| (index, 0)).collect();
    while let Some((index, depth)) = stack.pop() {
        let lane = &input.lanes[index];
        out.push(LaneRow {
            name: lane.name.clone(),
            branch: lane.branch.clone(),
            depth,
            main: lane.main,
            missing: lane.path.is_none(),
            dot: dot(lane),
        });
        stack.extend(
            children[index]
                .iter()
                .rev()
                .map(|&child| (child, depth + 1)),
        );
    }
    out
}

/// The index of the lane each lane is stacked `after`, when the sidebar lists
/// that lane. A lane that stacks on itself, directly or through others, has no
/// parent: the file refuses such cycles, but no lane is dropped for one.
fn parents(input: &ProjectInput<'_>) -> Vec<Option<usize>> {
    let parents: Vec<Option<usize>> = input
        .lanes
        .iter()
        .map(|lane| {
            let after = input
                .project
                .lanes
                .iter()
                .find(|file_lane| file_lane.name == lane.name)
                .and_then(|file_lane| file_lane.after.as_deref())?;
            input.lanes.iter().position(|other| other.name == after)
        })
        .collect();
    let count = parents.len();
    let in_cycle: Vec<bool> = (0..count)
        .map(|start| {
            let mut current = parents[start];
            for _ in 0..count {
                match current {
                    Some(parent) if parent == start => return true,
                    Some(parent) => current = parents[parent],
                    None => return false,
                }
            }
            false
        })
        .collect();
    parents
        .into_iter()
        .zip(in_cycle)
        .map(|(parent, cyclic)| if cyclic { None } else { parent })
        .collect()
}

fn dot(lane: &LaneInput) -> Dot {
    if lane.waiting {
        Dot::Waiting
    } else if lane.running {
        Dot::Running
    } else {
        Dot::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_project::Lane;

    /// A lane of the file, stacked `after` the named lane when given.
    fn file_lane(name: &str, after: Option<&str>) -> Lane {
        Lane {
            name: name.to_owned(),
            repo: "rvw".to_owned(),
            after: after.map(str::to_owned),
            terminals: Vec::new(),
        }
    }

    fn project(name: &str, lanes: Vec<Lane>) -> Project {
        Project {
            name: name.to_owned(),
            lanes,
            ..Project::default()
        }
    }

    fn lane(name: &str) -> LaneInput {
        LaneInput {
            name: name.to_owned(),
            branch: Some(format!("feat/{name}")),
            path: Some(PathBuf::from(format!("/w/{name}"))),
            main: false,
            waiting: false,
            running: false,
        }
    }

    fn input(project: &Project, lanes: Vec<LaneInput>, open: bool) -> ProjectInput<'_> {
        ProjectInput {
            project,
            lanes,
            open,
        }
    }

    fn names(group: &ProjectGroup) -> Vec<&str> {
        group.lanes.iter().map(|row| row.name.as_str()).collect()
    }

    fn depths(group: &ProjectGroup) -> Vec<usize> {
        group.lanes.iter().map(|row| row.depth).collect()
    }

    #[test]
    fn projects_and_lanes_stay_in_file_order() {
        let zeta = project("zeta", vec![file_lane("zz", None), file_lane("aa", None)]);
        let alpha = project("alpha", vec![file_lane("mm", None)]);
        let main = LaneInput {
            main: true,
            ..lane("main")
        };
        let sidebar = build(&[
            input(&zeta, vec![lane("zz"), lane("aa"), lane("found")], true),
            input(&alpha, vec![lane("mm"), main], true),
        ]);
        let got: Vec<&str> = sidebar.projects.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(got, ["zeta", "alpha"]);
        assert_eq!(names(&sidebar.projects[0]), ["zz", "aa", "found"]);
        assert_eq!(names(&sidebar.projects[1]), ["mm", "main"]);
        assert!(sidebar.projects[1].lanes[1].main);
        assert!(!sidebar.projects[1].lanes[0].main);
    }

    #[test]
    fn a_waiting_lane_does_not_move_its_project_or_lane() {
        let quiet = project("quiet", vec![file_lane("a", None)]);
        let busy = project("busy", vec![file_lane("b", None)]);
        let sidebar = build(&[
            input(&quiet, vec![lane("a")], true),
            input(
                &busy,
                vec![LaneInput {
                    waiting: true,
                    ..lane("b")
                }],
                true,
            ),
        ]);
        assert_eq!(sidebar.projects[0].name, "quiet");
        assert_eq!(sidebar.projects[1].name, "busy");
    }

    #[test]
    fn a_stacked_lane_sits_under_its_parent_with_depth() {
        // a; b after a; c; d after b; e after a. Siblings keep file order.
        let p = project(
            "rvw",
            vec![
                file_lane("a", None),
                file_lane("b", Some("a")),
                file_lane("c", None),
                file_lane("d", Some("b")),
                file_lane("e", Some("a")),
            ],
        );
        let sidebar = build(&[input(
            &p,
            vec![lane("a"), lane("b"), lane("c"), lane("d"), lane("e")],
            true,
        )]);
        let group = &sidebar.projects[0];
        assert_eq!(names(group), ["a", "b", "d", "e", "c"]);
        assert_eq!(depths(group), [0, 1, 2, 1, 0]);
    }

    #[test]
    fn a_child_listed_before_its_parent_still_sits_under_it() {
        let p = project(
            "rvw",
            vec![
                file_lane("child", Some("parent")),
                file_lane("parent", None),
            ],
        );
        let sidebar = build(&[input(&p, vec![lane("child"), lane("parent")], true)]);
        assert_eq!(names(&sidebar.projects[0]), ["parent", "child"]);
        assert_eq!(depths(&sidebar.projects[0]), [0, 1]);
    }

    #[test]
    fn an_unknown_after_is_a_root() {
        let p = project("rvw", vec![file_lane("x", Some("ghost"))]);
        let sidebar = build(&[input(&p, vec![lane("x")], true)]);
        assert_eq!(depths(&sidebar.projects[0]), [0]);
    }

    #[test]
    fn a_cycle_drops_no_lane() {
        let p = project(
            "rvw",
            vec![
                file_lane("a", Some("b")),
                file_lane("b", Some("a")),
                file_lane("self", Some("self")),
                file_lane("c", None),
            ],
        );
        let sidebar = build(&[input(
            &p,
            vec![lane("a"), lane("b"), lane("self"), lane("c")],
            true,
        )]);
        assert_eq!(names(&sidebar.projects[0]), ["a", "b", "self", "c"]);
        assert_eq!(depths(&sidebar.projects[0]), [0, 0, 0, 0]);
    }

    #[test]
    fn a_collapsed_project_lists_no_lanes_but_counts_the_waiting_ones() {
        let p = project("rvw", vec![file_lane("a", None), file_lane("b", None)]);
        let waiting = LaneInput {
            waiting: true,
            ..lane("a")
        };
        let sidebar = build(&[input(&p, vec![waiting.clone(), lane("b")], false)]);
        let group = &sidebar.projects[0];
        assert!(!group.open);
        assert!(group.lanes.is_empty());
        assert_eq!(group.waiting_lanes, 1);

        let open = build(&[input(&p, vec![waiting, lane("b")], true)]);
        assert_eq!(open.projects[0].waiting_lanes, 1);
        assert_eq!(names(&open.projects[0]), ["a", "b"]);
    }

    #[test]
    fn waiting_wins_over_running_and_idle_is_the_rest() {
        let cases = [
            (true, true, Dot::Waiting),
            (true, false, Dot::Waiting),
            (false, true, Dot::Running),
            (false, false, Dot::Idle),
        ];
        for (waiting, running, want) in cases {
            let p = project("rvw", vec![file_lane("a", None)]);
            let sidebar = build(&[input(
                &p,
                vec![LaneInput {
                    waiting,
                    running,
                    ..lane("a")
                }],
                true,
            )]);
            assert_eq!(
                sidebar.projects[0].lanes[0].dot, want,
                "waiting={waiting} running={running}"
            );
        }
    }

    #[test]
    fn a_lane_without_a_path_is_missing() {
        let p = project(
            "rvw",
            vec![file_lane("gone", None), file_lane("here", None)],
        );
        let gone = LaneInput {
            path: None,
            ..lane("gone")
        };
        let sidebar = build(&[input(&p, vec![gone, lane("here")], true)]);
        let rows = &sidebar.projects[0].lanes;
        assert!(rows[0].missing);
        assert!(!rows[1].missing);
    }

    #[test]
    fn a_lane_without_a_branch_keeps_none() {
        let p = project("rvw", vec![file_lane("a", None)]);
        let detached = LaneInput {
            branch: None,
            ..lane("a")
        };
        let sidebar = build(&[input(&p, vec![detached], true)]);
        assert_eq!(sidebar.projects[0].lanes[0].branch, None);
    }

    /// Two projects: `one` with a, b; `two` (collapsed) with c; `three` with d.
    fn sample() -> Sidebar {
        let one = project("one", vec![file_lane("a", None), file_lane("b", None)]);
        let two = project("two", vec![file_lane("c", None)]);
        let three = project("three", vec![file_lane("d", None)]);
        build(&[
            input(&one, vec![lane("a"), lane("b")], true),
            input(&two, vec![lane("c")], false),
            input(&three, vec![lane("d")], true),
        ])
    }

    #[test]
    fn step_moves_across_expanded_projects_and_skips_collapsed_ones() {
        let sidebar = sample();
        let table = [
            (Some(("one", "a")), 1, Some(("one", "b"))),
            (Some(("one", "b")), 1, Some(("three", "d"))),
            (Some(("three", "d")), 1, Some(("one", "a"))),
            (Some(("one", "a")), -1, Some(("three", "d"))),
            (Some(("three", "d")), -1, Some(("one", "b"))),
            (Some(("one", "a")), 2, Some(("three", "d"))),
            (Some(("one", "a")), 4, Some(("one", "b"))),
            (Some(("one", "a")), 0, Some(("one", "a"))),
            (Some(("one", "a")), -4, Some(("three", "d"))),
        ];
        for (current, delta, want) in table {
            let want = want.map(|(p, l)| (p.to_owned(), l.to_owned()));
            assert_eq!(
                step(&sidebar, current, delta),
                want,
                "from {current:?} by {delta}"
            );
        }
    }

    #[test]
    fn step_without_a_visible_current_lane() {
        let sidebar = sample();
        let owned = |(p, l): (&str, &str)| Some((p.to_owned(), l.to_owned()));
        assert_eq!(step(&sidebar, None, 1), owned(("one", "a")));
        assert_eq!(step(&sidebar, None, -1), owned(("three", "d")));
        assert_eq!(step(&sidebar, None, 0), None);
        assert_eq!(step(&sidebar, Some(("two", "c")), 1), owned(("one", "a")));
        assert_eq!(
            step(&sidebar, Some(("two", "c")), -1),
            owned(("three", "d"))
        );
        assert_eq!(step(&sidebar, Some(("one", "zz")), 1), owned(("one", "a")));
    }

    #[test]
    fn step_has_nothing_when_no_lane_is_visible() {
        let p = project("rvw", vec![file_lane("a", None)]);
        let empty = build(&[input(&p, vec![lane("a")], false)]);
        assert_eq!(step(&empty, None, 1), None);
        assert_eq!(step(&build(&[]), Some(("rvw", "a")), 1), None);
    }

    #[test]
    fn step_with_a_huge_delta_wraps_without_overflow() {
        // Three visible lanes: isize::MAX is 1 mod 3, isize::MIN is 1 mod 3.
        let sidebar = sample();
        let owned = |(p, l): (&str, &str)| Some((p.to_owned(), l.to_owned()));
        assert_eq!(
            step(&sidebar, Some(("one", "a")), isize::MAX),
            owned(("one", "b"))
        );
        assert_eq!(
            step(&sidebar, Some(("one", "a")), isize::MIN),
            owned(("one", "b"))
        );
    }
}
