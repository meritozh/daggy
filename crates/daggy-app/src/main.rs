//! Daggy is a DaisyDisk-style disk analyzer with native macOS chrome.

mod app_state;
mod locations;
mod root_view;
mod sf_symbol;
mod sunburst;
mod theme;

use gpui::{App, AppContext, KeyBinding, Menu, MenuItem, OsAction};

/// Navigate up one level (Escape).
#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct GoUp;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct SelectPrevious;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct SelectNext;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct ActivateSelection;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct OpenFolder;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct Quit;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct About;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct CopySelection;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct RevealInFinder;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct MinimizeWindow;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct ZoomWindow;

#[derive(Clone, PartialEq, Eq, Default, Debug, gpui::Action)]
#[action(namespace = daggy)]
pub struct CloseWindow;

pub fn app_menus() -> [Menu; 5] {
    [
        Menu::new("Daggy").items([
            MenuItem::action("About Daggy", About),
            MenuItem::separator(),
            MenuItem::action("Quit Daggy", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open…", OpenFolder),
            MenuItem::separator(),
            MenuItem::action("Show in Finder", RevealInFinder),
            MenuItem::separator(),
            MenuItem::action("Close", CloseWindow),
        ]),
        Menu::new("Edit").items([MenuItem::os_action(
            "Copy",
            CopySelection,
            OsAction::Copy,
        )]),
        Menu::new("Go").items([MenuItem::action("Enclosing Folder", GoUp)]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", MinimizeWindow),
            MenuItem::action("Zoom", ZoomWindow),
        ]),
    ]
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("escape", GoUp, None),
            KeyBinding::new("cmd-up", GoUp, None),
            KeyBinding::new("up", SelectPrevious, None),
            KeyBinding::new("down", SelectNext, None),
            KeyBinding::new("enter", ActivateSelection, None),
            KeyBinding::new("cmd-down", ActivateSelection, None),
            KeyBinding::new("cmd-o", OpenFolder, None),
            KeyBinding::new("cmd-c", CopySelection, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-m", MinimizeWindow, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(app_menus());

        let app_state = cx.new(app_state::AppState::new);
        cx.open_window(theme::window_options(cx), |window, cx| {
            let view = cx.new(|cx| root_view::RootView::new(app_state, window, cx));
            let focus = view.read(cx).focus_handle();
            focus.focus(window, cx);
            view
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::app_menus;

    #[test]
    fn menus_follow_macos_hig_structure() {
        let menus = app_menus();
        let names: Vec<&str> = menus.iter().map(|menu| menu.name.as_ref()).collect();
        assert_eq!(names, ["Daggy", "File", "Edit", "Go", "Window"]);
        assert_eq!(menus[1].items.len(), 5);
        assert_eq!(menus[2].items.len(), 1);
    }
}
