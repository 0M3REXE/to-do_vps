//! # Template structs, one per page or partial
//!
//! Each field becomes a variable in the matching `.html`. Askama compiles the
//! template at build time, so a wrong field is a build error — and editing a
//! template requires a rebuild before the change appears.

use askama::Template;

use super::Todo;

#[derive(Template)]
#[template(path = "index.html")]
pub(crate) struct IndexTemplate {
    pub(crate) todos: Vec<Todo>,
    pub(crate) open_count: i64,
    /// Shown in the header next to the logout button, so there is no doubt
    /// which account the list belongs to on a shared machine.
    pub(crate) email: String,
}

#[derive(Template)]
#[template(path = "todo_panel.html")]
pub(crate) struct PanelTemplate {
    pub(crate) todos: Vec<Todo>,
    pub(crate) open_count: i64,
}

#[derive(Template)]
#[template(path = "todo_list_update.html")]
pub(crate) struct TodoUpdateTemplate {
    pub(crate) todos: Vec<Todo>,
    pub(crate) open_count: i64,
}
