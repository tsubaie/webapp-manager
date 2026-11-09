// SPDX-License-Identifier: MIT

mod app;
mod i18n;

fn main() -> cosmic::iced::Result {
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);

    let settings = cosmic::app::Settings::default()
        .size(cosmic::iced::Size::new(760.0, 640.0))
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(420.0)
                .min_height(320.0),
        );

    cosmic::app::run::<app::AppModel>(settings, ())
}
