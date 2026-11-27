use super::*;
use crate::{TextEdit, TextStyle, col, el, text, text_input};

#[derive(Default)]
struct State {
    field: Option<TextEdit>,
    modal: bool,
    underline: bool,
    menu: Option<TextContext>,
    change_count: usize,
    patch_count: usize,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let temporary = TextEdit::new("mail");
    let editor = text_input(
        s.field.as_ref().unwrap_or(&temporary),
        TextStyle::new(16.0, 400.0),
    )
    .key("editor")
    .size(180.0, 38.0)
    .on_edit(|s: &mut State| s.field.get_or_insert_with(|| TextEdit::new("mail")))
    .on_input_change(|s: &mut State, _| s.change_count += 1)
    .on_input_paste(|s: &mut State, content, _| {
        s.patch_count += 1;
        s.underline = content.html().is_some();
        false
    });
    let draft = if s.modal {
        editor.modal(true, |_: &mut State| {})
    } else {
        editor
    };
    let root = col()
        .size(200.0, 100.0)
        .on_text_context(|s: &mut State, _, menu, _| s.menu = Some(menu))
        .child(draft)
        .child(text("abcd").key("label").size(180.0, 38.0).selectable(true))
        .child(
            text_input(&temporary, TextStyle::new(16.0, 400.0))
                .key("capture")
                .size(180.0, 20.0)
                .on_edit(|s: &mut State| s.field.get_or_insert_with(|| TextEdit::new("mail")))
                .text_context_enabled(false),
        );
    if s.modal && s.menu.is_some() {
        root.child(el().size(100.0, 40.0).modal(true, |_: &mut State| {}))
    } else {
        root
    }
}

fn click_menu(rt: &mut Runtime<State>, x: f32, y: f32) {
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
}

fn choose(rt: &mut Runtime<State>, command: TextCommand) {
    rt.update(move |s, tx| tx.edit_context(&s.menu.take().unwrap(), command));
    for _ in 0..4 {
        rt.frame(0.0);
    }
}

#[test]
fn the_first_modal_paste_uses_the_real_field_and_its_clipboard_hook() {
    let clipboard = crate::clipboard::memory();
    clipboard.set_content(crate::ClipboardContent::new(" copied").with_html("<u> copied</u>"));
    let mut rt = Runtime::new(
        State {
            modal: true,
            ..State::default()
        },
        view,
        (200.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&clipboard);
    rt.frame(0.0);
    click_menu(&mut rt, 12.0, 12.0);
    let menu = rt.state.menu.as_ref().unwrap();
    assert!(menu.is_editable() && menu.can_paste() && menu.can_select_all());
    assert!(!menu.can_copy());
    choose(&mut rt, TextCommand::SelectAll);
    assert_eq!(rt.state.field.as_ref().unwrap().selected(), "mail");
    click_menu(&mut rt, 12.0, 12.0);
    choose(&mut rt, TextCommand::Paste);
    assert_eq!(rt.state.field.as_ref().unwrap().text, " copied");
    assert_eq!(
        (
            rt.state.patch_count,
            rt.state.change_count,
            rt.state.underline
        ),
        (1, 1, true)
    );
}

#[test]
fn the_menu_key_and_mouse_cut_selected_text_and_copy_a_readonly_label() {
    let mut draft = TextEdit::new("中文 mail");
    draft.set_selection(0, "中文".len());
    draft.focused = true;
    let clipboard = crate::clipboard::memory();
    let mut rt = Runtime::new(
        State {
            field: Some(draft),
            ..State::default()
        },
        view,
        (200.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&clipboard);
    rt.frame(0.0);
    rt.key(KeyEvent {
        key: crate::Key::ContextMenu,
        mods: Default::default(),
    });
    rt.frame(0.0);
    assert!(rt.state.menu.as_ref().unwrap().can_cut());
    choose(&mut rt, TextCommand::Cut);
    assert_eq!(rt.state.field.as_ref().unwrap().text, " mail");
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("中文")
    );
    assert_eq!(rt.state.change_count, 1);

    click_menu(&mut rt, 7.0, 49.0);
    let menu = rt.state.menu.as_ref().unwrap();
    assert!(!menu.is_editable() && menu.can_copy() && !menu.can_paste());
    choose(&mut rt, TextCommand::Copy);
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("abcd")
    );
    click_menu(&mut rt, 12.0, 87.0);
    assert!(rt.state.menu.is_none(), "an IME capture can opt out");
}

#[test]
fn a_revealed_password_never_offers_copy_and_replaced_fields_reject_old_commands() {
    let mut draft = TextEdit::new("synthetic-only-password").secure();
    draft.set_revealed(true);
    draft.set_selection(0, draft.text.len());
    let clipboard = crate::clipboard::memory();
    clipboard.set_content(crate::ClipboardContent::new("clipboard"));
    let mut rt = Runtime::new(
        State {
            field: Some(draft),
            ..State::default()
        },
        view,
        (200.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&clipboard);
    rt.frame(0.0);
    click_menu(&mut rt, 12.0, 12.0);
    let menu = rt.state.menu.as_ref().unwrap();
    assert!(menu.can_paste() && menu.can_select_all());
    assert!(!menu.can_copy() && !menu.can_cut());
    assert!(!format!("{menu:?}").contains("synthetic-only-password"));
    rt.update(|s, tx| {
        tx.edit_context(s.menu.as_ref().unwrap(), TextCommand::Cut);
        tx.edit_context(s.menu.as_ref().unwrap(), TextCommand::Paste);
        s.field = Some(TextEdit::new("replacement"));
    });
    for _ in 0..4 {
        rt.frame(0.0);
    }
    assert_eq!(rt.state.field.as_ref().unwrap().text, "replacement");
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("clipboard")
    );
    assert_eq!(rt.state.patch_count, 0);
}

#[test]
fn the_clipboard_answer_cannot_paste_into_a_new_model_at_the_same_key() {
    let mut rt = Runtime::new(State::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click_menu(&mut rt, 12.0, 12.0);
    choose(&mut rt, TextCommand::Paste);
    let request = rt.read_requests.pop().unwrap();
    rt.update(|s, _| s.field = Some(TextEdit::new("another editor")));
    rt.read_result(request, crate::ClipboardContent::new("late reply"));
    for _ in 0..4 {
        rt.frame(0.0);
    }
    assert_eq!(rt.state.field.as_ref().unwrap().text, "another editor");
    assert_eq!(rt.state.patch_count, 0);
}

#[test]
fn editors_keep_their_own_handler_and_otherwise_win_over_ancestor_menus() {
    #[derive(Default)]
    struct Input {
        field: TextEdit,
        own_menu: bool,
        menu: Option<&'static str>,
    }
    fn render(s: &Input, _: &mut Cx<Input>) -> El {
        let input = text_input(&s.field, TextStyle::new(16.0, 400.0))
            .size(180.0, 36.0)
            .on_edit(|s: &mut Input| &mut s.field);
        let input = if s.own_menu {
            input.on_secondary_press(|s: &mut Input, _, _| s.menu = Some("own"))
        } else {
            input
        };
        col()
            .size(200.0, 100.0)
            .on_text_context(|s: &mut Input, _, _, _| s.menu = Some("text"))
            .child(
                el().size(190.0, 70.0)
                    .on_secondary_press(|s: &mut Input, _, _| s.menu = Some("ancestor"))
                    .child(input),
            )
    }
    let mut rt = Runtime::new(
        Input::default(),
        render,
        (200.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let right_click = |rt: &mut Runtime<Input>| {
        rt.secondary_down(12.0, 12.0);
        rt.secondary_up(12.0, 12.0);
        rt.frame(0.0);
    };
    right_click(&mut rt);
    assert_eq!(rt.state.menu, Some("text"));
    rt.update(|s, _| s.own_menu = true);
    rt.frame(0.0);
    right_click(&mut rt);
    assert_eq!(rt.state.menu, Some("own"));
}
