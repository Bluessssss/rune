//! Resolves the default text menu to the same editor and clipboard paths as keyboard shortcuts.
use super::*;
use crate::text_context::{Handler, InputTarget, Request, Target};
use crate::{TextCommand, TextContext};

impl<S: 'static> Runtime<S> {
    fn text_context_handler(&self, index: usize) -> Option<Handler> {
        let mut ancestors = std::iter::successors(Some(index), |&i| self.nodes[i].parent);
        ancestors.find_map(|i| self.nodes[i].handlers.on_text_context.clone())
    }

    pub(super) fn is_context_input(&self, i: usize) -> bool {
        let node = &self.nodes[i];
        !node.handlers.no_text_context
            && !node.a11y.disabled
            && node
                .input
                .as_ref()
                .is_some_and(|input| input.access.is_some())
            && self.can_focus(i)
    }

    pub(super) fn has_text_context(&self, i: usize) -> bool {
        (self.is_context_input(i) || self.is_selectable(i))
            && self.text_context_handler(i).is_some()
    }

    pub(super) fn open_text_context(
        &mut self,
        path: &str,
        point: PointerEvent,
        select_word: bool,
        tx: &mut Tx,
    ) -> bool {
        let Some(i) = self.live(path) else {
            return false;
        };
        let Some(handler) = self.text_context_handler(i) else {
            return false;
        };
        let target = if self.is_context_input(i) {
            let access = self.nodes[i]
                .input
                .as_ref()
                .and_then(|input| input.access.clone())
                .unwrap();
            if self.focused.as_deref() != Some(path) {
                self.set_focus(Some(path.to_owned()), !select_word);
            }
            let editor = access(states!(self));
            editor.break_history_group();
            Target::Input {
                target: InputTarget {
                    path: path.to_owned(),
                    history: editor.weak_history(),
                },
                selected: editor.anchor != editor.focus,
                nonempty: !editor.text.is_empty(),
                secure: editor.is_secure(),
            }
        } else if self.is_selectable(i) {
            if select_word {
                self.select_word_for_menu(i, point.window_x, point.window_y);
            }
            let text = super::selectable::range_in(&self.selection_ranges(), i)
                .and_then(|_| self.copy_text());
            Target::ReadOnly(text)
        } else {
            return false;
        };
        handler(states!(self), point.window(), TextContext(target), tx);
        true
    }

    fn live_context_input(&self, target: &InputTarget) -> bool {
        self.live(&target.path).is_some_and(|i| {
            self.is_context_input(i)
                && self.nodes[i]
                    .input
                    .as_ref()
                    .is_some_and(|input| input.edit.matches_history(&target.history))
        })
    }

    pub(super) fn run_edit_context(&mut self, request: Request, tx: &mut Tx) -> bool {
        let (target, command) = match request {
            Request::Paste(target, content) => {
                // The clipboard may arrive after focus moved, a pane closed, or a new editor took
                // the same key. A late paste belongs to none of those editors.
                return self.focused.as_deref() == Some(target.path.as_str())
                    && self.live_context_input(&target)
                    && self.apply_paste(content, tx);
            }
            Request::Command(target, command) => (target, command),
        };
        if !self.live_context_input(&target) {
            return false;
        }
        if self.focused.as_deref() != Some(target.path.as_str()) {
            self.set_focus(Some(target.path.clone()), self.keyboard_focus);
        }
        let key = match command {
            TextCommand::Cut => 'x',
            TextCommand::Copy => 'c',
            TextCommand::SelectAll => 'a',
            TextCommand::Paste => {
                tx.read_clipboard(move |_: &mut S, content, tx| {
                    tx.edit_context
                        .push(Request::Paste(target, content.clone()));
                });
                return true;
            }
        };
        self.edit_key(
            &KeyEvent {
                key: crate::Key::Command(key),
                mods: Default::default(),
            },
            tx,
        )
    }
}

#[cfg(test)]
#[path = "text_context_tests.rs"]
mod tests;
