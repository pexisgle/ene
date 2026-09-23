use ene_desktop::ui::presentation::{discard_secret_input, erase_surface_copies};
use ene_desktop_ui::{ChatWindow, ManagementWindow};
use slint::platform::{
    Platform, WindowAdapter,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
use slint::{ComponentHandle, LogicalSize};
use std::rc::Rc;

struct TestPlatform;
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}
#[test]
fn independent_surfaces_preserve_drafts_and_erase_native_trees() {
    slint::platform::set_platform(Box::new(TestPlatform)).expect("test backend");
    let chat = ChatWindow::new().expect("chat");
    let management = ManagementWindow::new().expect("management");
    chat.window().set_size(LogicalSize::new(1100., 760.));
    management.window().set_size(LogicalSize::new(1000., 720.));
    chat.show().expect("show chat");
    management.show().expect("show management");
    chat.set_draft("chat-only".into());
    chat.set_workspace_draft("workspace-only".into());
    chat.set_instruction_draft("task-only".into());
    chat.set_tasks_open(true);
    management.set_page(3);
    management.hide().expect("close management");
    assert_eq!(chat.get_draft(), "chat-only");
    assert_eq!(chat.get_workspace_draft(), "workspace-only");
    assert_eq!(chat.get_instruction_draft(), "task-only");
    chat.set_tasks_open(false);
    management.show().expect("reopen management");
    assert_eq!(management.get_page(), 3);
    assert_eq!(chat.get_draft(), "chat-only");
    management.set_secret_draft("fixture-private-secret".into());
    assert!(discard_secret_input(&management));
    assert!(management.get_secret_draft().is_empty());
    // Leave a deleted character in native undo history while the public draft
    // is already empty: assigning "" alone cannot prove this copy was removed.
    chat.set_draft("".into());
    chat.window().take_snapshot().expect("layout");
    let position = slint::LogicalPosition::new(100., 700.);
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "x".into() });
    assert_eq!(chat.get_draft(), "x");
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Backspace.into(),
        });
    assert!(chat.get_draft().is_empty());
    management.hide().expect("hidden erasure");
    assert!(erase_surface_copies(&chat, &management));
    assert!(chat.get_draft().is_empty());
    assert!(chat.get_workspace_draft().is_empty());
    assert!(chat.get_instruction_draft().is_empty());
    assert!(management.get_secret_draft().is_empty());
    assert!(!management.get_can_confirm());
    assert!(chat.get_content_live());
    assert!(management.get_content_live());
    chat.window().take_snapshot().expect("recreated scene");
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Control.into(),
        });
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "z".into() });
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
            text: slint::platform::Key::Control.into(),
        });
    assert!(
        chat.get_draft().is_empty(),
        "native undo must not resurrect erased input"
    );
    chat.window().set_size(LogicalSize::new(720., 540.));
    chat.set_tasks_open(true);
    chat.set_progress_summary("2 tasks".into());
    chat.window().take_snapshot().expect("compact layout");
    let position = slint::LogicalPosition::new(100., 235.);
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    chat.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "y".into() });
    assert_eq!(
        chat.get_draft(),
        "y",
        "bottom task pane must not cover the composer"
    );
}
