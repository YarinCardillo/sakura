//! What the base chooser of "New task" offers: the repository's own branches first, and apart from them
//! the branches other lavori work on, each under its lavoro's name. A lavoro's branch is one of the
//! repository's too, but it is someone's work in progress and not a line to start from by habit: mixed
//! with the others it reads as noise.

/// The branches a new lavoro can start from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Offered {
    /// The repository's own branches, the project's base first.
    pub own: Vec<String>,
    /// The branches other lavori work on, with the lavoro's name: `(branch, lavoro)`.
    pub from_tasks: Vec<(String, String)>,
}

/// Sorts `branches` (what the repository has) into its own and the ones of `tasks` (`(lavoro, branch)`).
/// The project's `base` is first among its own, listed once, whoever works on it.
pub fn group(base: Option<&str>, branches: &[String], tasks: &[(String, String)]) -> Offered {
    let mut offered = Offered {
        own: base.iter().map(|base| base.to_string()).collect(),
        from_tasks: Vec::new(),
    };
    for branch in branches {
        if Some(branch.as_str()) == base || offered.own.contains(branch) {
            continue;
        }
        let task = tasks.iter().find(|(_, of_task)| of_task == branch);
        match task {
            Some((task, _)) => offered.from_tasks.push((branch.clone(), task.clone())),
            None => offered.own.push(branch.clone()),
        }
    }
    offered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn tasks(tasks: &[(&str, &str)]) -> Vec<(String, String)> {
        let owned = |(task, branch): &(&str, &str)| (task.to_string(), branch.to_string());
        tasks.iter().map(owned).collect()
    }

    #[test]
    fn group_other_tasks_branches_are_apart_under_the_task_s_name() {
        let branches = names(&["main", "sakura/fix-login", "dev", "box/m1"]);
        let tasks = tasks(&[("fix-login", "sakura/fix-login"), ("m1", "box/m1")]);
        let offered = group(Some("main"), &branches, &tasks);
        assert_eq!(offered.own, names(&["main", "dev"]));
        let from_tasks = [("sakura/fix-login", "fix-login"), ("box/m1", "m1")];
        let from_tasks: Vec<_> = from_tasks
            .iter()
            .map(|(branch, task)| (branch.to_string(), task.to_string()))
            .collect();
        assert_eq!(offered.from_tasks, from_tasks);
    }

    #[test]
    fn group_the_project_s_base_is_first_and_listed_once_even_when_a_task_works_on_it() {
        let branches = names(&["dev", "main"]);
        let tasks = tasks(&[("on-main", "main")]);
        let offered = group(Some("main"), &branches, &tasks);
        assert_eq!(offered.own, names(&["main", "dev"]));
        assert!(offered.from_tasks.is_empty());
    }

    #[test]
    fn group_a_task_s_branch_the_repository_does_not_have_is_not_offered() {
        let tasks = tasks(&[("gone", "sakura/gone")]);
        let offered = group(None, &names(&["main"]), &tasks);
        assert_eq!(offered.own, names(&["main"]));
        assert!(offered.from_tasks.is_empty());
    }
}
