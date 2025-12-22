// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cosmic::app::{Core, Task};
use cosmic::cosmic_config::{Config as CosmicConfig, CosmicConfigEntry};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::iced::{time, Limits, Subscription, Length};
use cosmic::widget::{self, container, text};
use cosmic::{theme, Application, Element, Theme};

use crate::config::Config;
use crate::fl;
use crate::space::{self, SpaceInfo};
use crate::udisks::{self, DriveInfo};

/// Combined drive and space data for display.
#[derive(Debug, Clone)]
pub struct DriveStatus {
    pub info: DriveInfo,
    pub space: SpaceInfo,
}

/// Tracks alert state for a drive to implement cooldown.
#[derive(Debug, Clone)]
struct AlertState {
    last_alerted: Instant,
    was_over_threshold: bool,
}

pub struct CargoWatch {
    core: Core,
    popup: Option<Id>,
    config: Config,
    config_handler: Option<CosmicConfig>,
    drives: Vec<DriveStatus>,
    alert_states: HashMap<PathBuf, AlertState>,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    Tick,
    OpenFileManager(PathBuf),
    TogglePanelDrive(String, bool),
    ToggleDriveAlert(String, bool),
    SetDriveThreshold(String, u8),
    #[allow(dead_code)]
    ConfigChanged(Config),
}

#[allow(mismatched_lifetime_syntaxes)]
impl Application for CargoWatch {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.vintagetechie.CosmicExtAppletCargoWatch";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let (config, config_handler) = match CosmicConfig::new(Self::APP_ID, Config::VERSION) {
            Ok(handler) => {
                let config = Config::get_entry(&handler).unwrap_or_default();
                (config, Some(handler))
            }
            Err(why) => {
                eprintln!("failed to load config: {why}");
                (Config::default(), None)
            }
        };

        let mut app = CargoWatch {
            core,
            popup: None,
            config,
            config_handler,
            drives: Vec::new(),
            alert_states: HashMap::new(),
        };

        // Initial drive scan
        app.refresh_drives();

        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<Self::Message> {
        // Get drives that should show on panel
        let panel_drives: Vec<_> = self
            .drives
            .iter()
            .filter(|d| self.is_on_panel(&d.info.mount_point))
            .collect();

        if panel_drives.is_empty() {
            // Just show icon if no drives selected for panel display
            self.core
                .applet
                .icon_button_from_handle(
                    widget::icon::from_name("com.vintagetechie.CosmicExtAppletCargoWatch")
                        .fallback(Some(widget::icon::IconFallback::Names(vec![
                            "drive-harddisk-symbolic".into(),
                        ])))
                        .into(),
                )
                .on_press(Message::TogglePopup)
                .into()
        } else {
            // Build content based on panel orientation
            let data = if self.core.applet.is_horizontal() {
                let mut row = widget::row::Row::new()
                    .spacing(8)
                    .align_y(cosmic::iced::Alignment::Center);

                for drive in &panel_drives {
                    let name = drive.info.display_name();
                    let pct = drive.space.percent_used();
                    let mount_str = drive.info.mount_point.display().to_string();
                    let alert_config = self.config.get_drive_alert(&mount_str);
                    let is_warning = pct >= alert_config.threshold;

                    let pct_text = if is_warning {
                        text(format!("{pct}%")).class(theme::Text::Custom(danger_text_style))
                    } else {
                        text(format!("{pct}%"))
                    };

                    let drive_display = widget::row::Row::new()
                        .spacing(4)
                        .align_y(cosmic::iced::Alignment::Center)
                        .push(text(name).size(14))
                        .push(pct_text.size(14));

                    row = row.push(drive_display);
                }
                Element::from(row)
            } else {
                // Vertical panel - stack drives
                let mut col = widget::column::Column::new()
                    .spacing(4)
                    .align_x(cosmic::iced::Alignment::Center);

                for drive in &panel_drives {
                    let name = drive.info.display_name();
                    let pct = drive.space.percent_used();
                    let mount_str = drive.info.mount_point.display().to_string();
                    let alert_config = self.config.get_drive_alert(&mount_str);
                    let is_warning = pct >= alert_config.threshold;

                    let pct_text = if is_warning {
                        text(format!("{pct}%")).class(theme::Text::Custom(danger_text_style))
                    } else {
                        text(format!("{pct}%"))
                    };

                    col = col.push(
                        widget::column::Column::new()
                            .align_x(cosmic::iced::Alignment::Center)
                            .push(text(name).size(12))
                            .push(pct_text.size(12)),
                    );
                }
                Element::from(col)
            };

            let button = widget::button::custom(data)
                .class(theme::Button::AppletIcon)
                .on_press(Message::TogglePopup);

            widget::autosize::autosize(button, widget::Id::unique()).into()
        }
    }

    fn view_window(&self, _id: Id) -> Element<Self::Message> {
        let mut content = widget::column::Column::new().spacing(8).padding(12);

        if self.drives.is_empty() {
            content = content.push(text(fl!("no-drives")));
        } else {
            for drive in &self.drives {
                let name = drive.info.display_name();
                let pct = drive.space.percent_used();
                let used = space::format_bytes(drive.space.used);
                let total = space::format_bytes(drive.space.total);
                let mount = drive.info.mount_point.clone();
                let mount_str = mount.display().to_string();

                let alert_config = self.config.get_drive_alert(&mount_str);
                let is_warning = pct >= alert_config.threshold;
                let is_on_panel = self.is_on_panel(&mount);

                // Clones for closures
                let mount_str_panel = mount_str.clone();
                let mount_str_alert = mount_str.clone();
                let mount_str_threshold = mount_str.clone();

                // Checkbox for panel visibility
                let panel_toggle = widget::checkbox(fl!("show-on-panel"), is_on_panel)
                    .on_toggle(move |checked| {
                        Message::TogglePanelDrive(mount_str_panel.clone(), checked)
                    })
                    .size(14);

                // Checkbox for alert enable/disable
                let alert_toggle = widget::checkbox(fl!("enable-alerts"), alert_config.enabled)
                    .on_toggle(move |checked| {
                        Message::ToggleDriveAlert(mount_str_alert.clone(), checked)
                    })
                    .size(14);

                // Threshold slider
                let threshold_row = widget::row::Row::new()
                    .spacing(8)
                    .align_y(cosmic::iced::Alignment::Center)
                    .push(text(fl!("threshold")).size(12))
                    .push(
                        widget::slider(50..=99, alert_config.threshold, move |val| {
                            Message::SetDriveThreshold(mount_str_threshold.clone(), val)
                        })
                        .width(Length::Fixed(100.0)),
                    )
                    .push(text(format!("{}%", alert_config.threshold)).size(12));

                let header_row = widget::row::Row::new()
                    .push(text(name).size(14))
                    .push(widget::horizontal_space())
                    .push(text(format!("{used} / {total}")).size(12));

                let bar = widget::progress_bar(0.0..=100.0, pct as f32).height(8);

                let bar_widget: Element<Self::Message> = if is_warning {
                    bar.class(theme::ProgressBar::Danger).into()
                } else {
                    bar.into()
                };

                let footer_row = widget::row::Row::new()
                    .push(text(drive.info.mount_point.display().to_string()).size(11))
                    .push(widget::horizontal_space())
                    .push(text(format!("{pct}%")).size(12));

                // Info section is clickable to open file manager
                let info_content = widget::column::Column::new()
                    .spacing(4)
                    .push(header_row)
                    .push(bar_widget)
                    .push(footer_row);

                let clickable_info = widget::mouse_area(info_content)
                    .on_press(Message::OpenFileManager(mount));

                // Settings row with both checkboxes
                let settings_row = widget::row::Row::new()
                    .spacing(16)
                    .push(panel_toggle)
                    .push(alert_toggle);

                // Card contains clickable info + divider + settings
                let card_content = widget::column::Column::new()
                    .spacing(6)
                    .push(clickable_info)
                    .push(widget::divider::horizontal::light())
                    .push(settings_row)
                    .push(threshold_row);

                let card = container(card_content)
                    .padding(8)
                    .width(Length::Fill)
                    .class(theme::Container::Card);

                content = content.push(card);
            }
        }

        self.core.applet.popup_container(content).into()
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    destroy_popup(p)
                } else {
                    let new_id = Id::unique();
                    self.popup.replace(new_id);
                    let mut popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    popup_settings.positioner.size_limits = Limits::NONE
                        .max_width(372.0)
                        .min_width(300.0)
                        .min_height(100.0)
                        .max_height(600.0);
                    get_popup(popup_settings)
                };
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
            Message::Tick => {
                self.refresh_drives();
                self.check_alerts();
            }
            Message::OpenFileManager(path) => {
                if let Err(why) = open::that(&path) {
                    eprintln!("failed to open file manager for {}: {why}", path.display());
                }
            }
            Message::TogglePanelDrive(mount, show) => {
                if show {
                    // Only add if not already matched (exact or prefix)
                    if !self.is_on_panel(Path::new(&mount)) {
                        self.config.panel_drives.push(mount);
                    }
                } else {
                    // Remove exact match
                    self.config.panel_drives.retain(|m| m != &mount);
                    // Also remove any prefix that was matching this path
                    // (e.g., remove "/home" when unchecking "/home/john")
                    self.config.panel_drives.retain(|m| {
                        !(m == "/home" && mount.starts_with("/home"))
                    });
                }
                self.save_config();
            }
            Message::ToggleDriveAlert(mount, enabled) => {
                let mut alert_config = self.config.get_drive_alert(&mount);
                alert_config.enabled = enabled;
                self.config.drive_alerts.insert(mount, alert_config);
                self.save_config();
            }
            Message::SetDriveThreshold(mount, threshold) => {
                let mut alert_config = self.config.get_drive_alert(&mount);
                alert_config.threshold = threshold;
                self.config.drive_alerts.insert(mount, alert_config);
                self.save_config();
            }
            Message::ConfigChanged(config) => {
                self.config = config;
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        time::every(Duration::from_secs(self.config.poll_interval)).map(|_| Message::Tick)
    }

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}

impl CargoWatch {
    /// Returns true if the given mount point should be shown on the panel.
    fn is_on_panel(&self, mount_point: &Path) -> bool {
        let mount_str = mount_point.display().to_string();
        self.config.panel_drives.iter().any(|m| {
            m == &mount_str
                || (m == "/home" && mount_str.starts_with("/home"))
        })
    }

    /// Saves the current config to disk.
    fn save_config(&self) {
        if let Some(ref handler) = self.config_handler {
            if let Err(why) = self.config.write_entry(handler) {
                eprintln!("failed to save config: {why}");
            }
        }
    }

    /// Refreshes drive list and space info.
    fn refresh_drives(&mut self) {
        let all_drives = match udisks::enumerate_drives() {
            Ok(drives) => drives,
            Err(why) => {
                eprintln!("failed to enumerate drives: {why}");
                return;
            }
        };

        // Filter to configured drives, or all non-removable if none configured
        let filtered: Vec<_> = if self.config.monitored_drives.is_empty() {
            all_drives.into_iter().filter(|d| !d.removable).collect()
        } else {
            all_drives
                .into_iter()
                .filter(|d| {
                    self.config
                        .monitored_drives
                        .iter()
                        .any(|m| d.mount_point == std::path::Path::new(m))
                })
                .collect()
        };

        // Get space info for each drive
        self.drives = filtered
            .into_iter()
            .filter_map(|info| {
                match space::get_space_info(&info.mount_point) {
                    Ok(space) => Some(DriveStatus { info, space }),
                    Err(why) => {
                        eprintln!(
                            "failed to get space for {}: {why}",
                            info.mount_point.display()
                        );
                        None
                    }
                }
            })
            .collect();
    }

    /// Checks drives against alert threshold and sends notifications.
    fn check_alerts(&mut self) {
        let now = Instant::now();
        let cooldown = Duration::from_secs(self.config.alert_cooldown);

        // Collect alerts to send (avoids borrow conflict)
        let mut alerts_to_send: Vec<(String, u8)> = Vec::new();

        for drive in &self.drives {
            let path = &drive.info.mount_point;
            let mount_str = path.display().to_string();
            let alert_config = self.config.get_drive_alert(&mount_str);

            // Skip if alerts disabled for this drive
            if !alert_config.enabled {
                continue;
            }

            let pct = drive.space.percent_used();
            let over_threshold = pct >= alert_config.threshold;

            let state = self.alert_states.entry(path.clone()).or_insert(AlertState {
                last_alerted: Instant::now() - cooldown - Duration::from_secs(1),
                was_over_threshold: false,
            });

            // Alert if:
            // 1. Currently over threshold AND
            // 2. Either just crossed threshold OR cooldown expired
            let crossed_threshold = over_threshold && !state.was_over_threshold;
            let cooldown_expired = now.duration_since(state.last_alerted) >= cooldown;

            if over_threshold && (crossed_threshold || cooldown_expired) {
                alerts_to_send.push((drive.info.display_name(), pct));
                state.last_alerted = now;
            }

            state.was_over_threshold = over_threshold;
        }

        for (name, pct) in alerts_to_send {
            Self::send_alert(&name, pct);
        }
    }

    fn send_alert(name: &str, pct: u8) {
        use notify_rust::{Notification, Urgency};

        let summary = fl!("alert-title");
        let body = fl!("alert-body", drive = name, percent = pct.to_string());

        if let Err(why) = Notification::new()
            .summary(&summary)
            .body(&body)
            .icon("drive-harddisk")
            .urgency(Urgency::Critical)
            .show()
        {
            eprintln!("failed to send notification: {why}");
        }
    }
}

/// Returns a text style using the theme's destructive color.
fn danger_text_style(theme: &Theme) -> cosmic::iced_widget::text::Style {
    cosmic::iced_widget::text::Style {
        color: Some(theme.cosmic().destructive_color().into()),
    }
}
