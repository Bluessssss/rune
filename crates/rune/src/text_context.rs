//! Presenting the default text context menu in the app's own UI.
use std::{fmt, rc::Rc, sync::Weak};

use crate::{
    El, PointerPress, Tx,
    edit::History,
    states::{States, downcast},
};

/// A standard command for the text under a context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextCommand {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The live text under a context menu. Apps can inspect available commands but never see an
/// editor's selection, including when a secure field temporarily shows its value.
#[derive(Clone, PartialEq)]
pub struct TextContext(pub(crate) Target);

#[derive(Clone, PartialEq)]
pub(crate) enum Target {
    ReadOnly(Option<String>),
    Input {
        target: InputTarget,
        selected: bool,
        nonempty: bool,
        secure: bool,
    },
}

#[derive(Clone)]
pub(crate) struct InputTarget {
    pub path: String,
    pub history: Weak<History>,
}

impl PartialEq for InputTarget {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.history.ptr_eq(&other.history)
    }
}

impl fmt::Debug for TextContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextContext")
            .field("editable", &self.is_editable())
            .field("can_copy", &self.can_copy())
            .finish_non_exhaustive()
    }
}

impl TextContext {
    pub fn is_editable(&self) -> bool {
        matches!(self.0, Target::Input { .. })
    }
    pub fn can_cut(&self) -> bool {
        matches!(
            self.0,
            Target::Input {
                selected: true,
                secure: false,
                ..
            }
        )
    }
    pub fn can_copy(&self) -> bool {
        matches!(self.0, Target::ReadOnly(Some(_))) || self.can_cut()
    }
    pub fn can_paste(&self) -> bool {
        self.is_editable()
    }
    pub fn can_select_all(&self) -> bool {
        matches!(self.0, Target::Input { nonempty: true, .. })
    }
}

#[derive(Clone)]
pub(crate) enum Request {
    Command(InputTarget, TextCommand),
    Paste(InputTarget, crate::ClipboardContent),
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TextContextRequest")
    }
}

pub(crate) type Handler = Rc<dyn Fn(States<'_>, PointerPress, TextContext, &mut Tx)>;

impl El {
    /// Present the default text menu for selectable labels and editors in this subtree. Links,
    /// editors with their own secondary-press handlers, and rows with a menu keep those menus.
    /// A containing presenter also hears presses inside modals.
    pub fn on_text_context<S: 'static>(
        mut self,
        f: impl Fn(&mut S, PointerPress, TextContext, &mut Tx) + 'static,
    ) -> Self {
        self.handlers.on_text_context = Some(Rc::new(move |s, at, menu, tx| {
            f(downcast::<S>(s), at, menu, tx)
        }));
        self
    }

    /// Disable the default context menu on this editor, for example on an invisible text input
    /// used only to forward IME and keyboard events to a remote screen.
    pub fn text_context_enabled(mut self, enabled: bool) -> Self {
        self.handlers.no_text_context = !enabled;
        self
    }
}

impl Tx {
    /// Apply a choice after dismissing the rendered menu. If the original editor is gone,
    /// replaced, or no longer usable when the command runs, it is ignored.
    pub fn edit_context(&mut self, context: &TextContext, command: TextCommand) {
        let enabled = match command {
            TextCommand::Cut => context.can_cut(),
            TextCommand::Copy => context.can_copy(),
            TextCommand::Paste => context.can_paste(),
            TextCommand::SelectAll => context.can_select_all(),
        };
        if !enabled {
            return;
        }
        match &context.0 {
            Target::ReadOnly(Some(text)) => self.copy(text.clone()),
            Target::Input { target, .. } => self
                .edit_context
                .push(Request::Command(target.clone(), command)),
            Target::ReadOnly(None) => {}
        }
    }
}
