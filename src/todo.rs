//! Sprint 5: quick task checklist operations.
//!
//! The list is stored in `state.json` with stable numeric ids. All mutations
//! return whether anything changed so the caller knows when to persist.

use crate::state::TodoItem;

pub const MAX_TODOS: usize = 50;
pub const MAX_TEXT_CHARS: usize = 120;

pub fn add(todos: &mut Vec<TodoItem>, text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() || todos.len() >= MAX_TODOS {
        return None;
    }
    let text: String = text.chars().take(MAX_TEXT_CHARS).collect();
    let id = todos.iter().map(|todo| todo.id).max().unwrap_or(0) + 1;
    todos.push(TodoItem {
        id,
        text,
        completed: false,
    });
    Some(id)
}

pub fn toggle(todos: &mut [TodoItem], id: u64) -> bool {
    if let Some(todo) = todos.iter_mut().find(|todo| todo.id == id) {
        todo.completed = !todo.completed;
        true
    } else {
        false
    }
}

pub fn remove(todos: &mut Vec<TodoItem>, id: u64) -> bool {
    let before = todos.len();
    todos.retain(|todo| todo.id != id);
    todos.len() != before
}

pub fn clear_completed(todos: &mut Vec<TodoItem>) -> bool {
    let before = todos.len();
    todos.retain(|todo| !todo.completed);
    todos.len() != before
}

pub fn move_item(todos: &mut Vec<TodoItem>, from: usize, to: usize) -> bool {
    if from >= todos.len() || to >= todos.len() || from == to {
        return false;
    }
    let item = todos.remove(from);
    todos.insert(to, item);
    true
}

pub fn completed_count(todos: &[TodoItem]) -> usize {
    todos.iter().filter(|todo| todo.completed).count()
}

pub fn summary(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        "No tasks yet".to_string()
    } else {
        format!("{} / {} done", completed_count(todos), todos.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<TodoItem> {
        vec![
            TodoItem {
                id: 1,
                text: "one".into(),
                completed: false,
            },
            TodoItem {
                id: 2,
                text: "two".into(),
                completed: true,
            },
            TodoItem {
                id: 3,
                text: "three".into(),
                completed: false,
            },
        ]
    }

    #[test]
    fn add_trims_and_assigns_stable_ids() {
        let mut todos = list();
        assert_eq!(add(&mut todos, "  four  "), Some(4));
        assert_eq!(todos.last().unwrap().text, "four");
        assert_eq!(add(&mut todos, "   "), None);
        assert_eq!(todos.len(), 4);
    }

    #[test]
    fn ids_stay_unique_after_removal() {
        let mut todos = list();
        assert!(remove(&mut todos, 3));
        let id = add(&mut todos, "next").unwrap();
        assert_eq!(todos.iter().filter(|todo| todo.id == id).count(), 1);
    }

    #[test]
    fn toggle_and_clear_completed() {
        let mut todos = list();
        assert!(toggle(&mut todos, 1));
        assert_eq!(completed_count(&todos), 2);
        assert!(clear_completed(&mut todos));
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].id, 3);
        assert!(!clear_completed(&mut todos));
    }

    #[test]
    fn move_reorders_only_valid_targets() {
        let mut todos = list();
        assert!(move_item(&mut todos, 0, 2));
        assert_eq!(
            todos.iter().map(|todo| todo.id).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        assert!(!move_item(&mut todos, 0, 3));
        assert!(!move_item(&mut todos, 1, 1));
    }

    #[test]
    fn summary_reports_progress() {
        assert_eq!(summary(&[]), "No tasks yet");
        assert_eq!(summary(&list()), "1 / 3 done");
    }
}
