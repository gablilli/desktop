use win32_notif::{
    NotificationBuilder, ToastsNotifier,
    notification::{actions::{ActionButton, Input, action::ActivationType, input::Selection}, visual::{Image, Placement, Text, text::HintStyle}},
};

pub fn send_toast() {
    let notifier = ToastsNotifier::new("Cloudreve.Sync").unwrap();

    let notif = NotificationBuilder::new()
        .visual(Image::create(0,"https://unsplash.it/64?image=669").with_placement(Placement::AppLogoOverride))
        .visual(
            Text::create(1, "Local change conflicted with remote")
                .with_align_center(true)
                .with_wrap(true)
                .with_style(HintStyle::Title),
        )
        .visual(
            Text::create(2, "SomeFile.docx")
                .with_align_center(true)
                .with_wrap(true)
                .with_style(HintStyle::Body),
        )
        .actions(vec![
            Box::new(Input::create_selection_input("selection", "Select an action", "Select an action", vec![
                Selection::new("keep_local", "Keep local"),
                Selection::new("overwrite_remote", "Overwrite remote"),
            ])),
            Box::new(ActionButton::create("Resolve").with_id("resolve").with_tooltip("Resolve the selected action")),
            Box::new(ActionButton::create("Dismiss").with_id("action=dismiss")),
        ])
        .build(0, &notifier, "01", "readme")
        .unwrap();

    notif.show().unwrap();
}
