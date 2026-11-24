use crate::theme;
use gpui::{div, AnyElement, ParentElement as _, Styled as _};

pub struct AccountRow {
    pub check: Option<AnyElement>,
    pub name: AnyElement,
    pub caption: Option<AnyElement>,
    pub actions: Vec<AnyElement>,
    pub notes: Vec<AnyElement>,
}

impl AccountRow {
    pub fn layout(self) -> gpui::Div {
        let check_width = theme::CONTROL_HEIGHT;
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme::GAP_TIGHT)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(theme::GAP)
                    .child(div().w(check_width).flex_none().children(self.check))
                    .child(
                        div()
                            .w(theme::kit::SETUP_LOGIN_ACTION_WIDTH)
                            .flex_none()
                            .child(self.name),
                    )
                    .children(
                        self.caption
                            .map(|caption| div().flex_1().min_w_0().child(caption)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_end()
                            .gap(theme::GAP_SMALL)
                            .children(self.actions),
                    ),
            )
            .child(
                div()
                    .pl(check_width + theme::GAP)
                    .flex()
                    .flex_col()
                    .gap(theme::GAP_TIGHT)
                    .children(self.notes),
            )
    }
}
