use anyhow::Result;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use super::*;
    use tray_icon::{
        Icon, TrayIcon, TrayIconBuilder,
        menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    pub struct AgentTray {
        _icon: TrayIcon,
        open: MenuItem,
        pause: CheckMenuItem,
        quit: MenuItem,
        launch_url: String,
    }

    pub enum TrayAction {
        Open(String),
        SetPaused(bool),
        Quit,
    }

    impl AgentTray {
        pub fn new(launch_url: String, paused: bool) -> Result<Self> {
            let menu = Menu::new();
            let open = MenuItem::new("打开管理页", true, None);
            let pause = CheckMenuItem::new("暂停同步", true, paused, None);
            let separator = PredefinedMenuItem::separator();
            let quit = MenuItem::new("退出 EPD Agent", true, None);
            menu.append_items(&[&open, &pause, &separator, &quit])?;
            let icon = TrayIconBuilder::new()
                .with_tooltip("EPD Agent")
                .with_menu(Box::new(menu))
                .with_icon(agent_icon()?)
                .with_icon_as_template(cfg!(target_os = "macos"))
                .build()?;
            Ok(Self {
                _icon: icon,
                open,
                pause,
                quit,
                launch_url,
            })
        }

        pub fn action(&self, event: &MenuEvent) -> Option<TrayAction> {
            if event.id == *self.open.id() {
                Some(TrayAction::Open(self.launch_url.clone()))
            } else if event.id == *self.pause.id() {
                Some(TrayAction::SetPaused(self.pause.is_checked()))
            } else if event.id == *self.quit.id() {
                Some(TrayAction::Quit)
            } else {
                None
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn agent_icon() -> Result<Icon> {
        const SIZE: usize = 36;
        const SAMPLES: usize = 4;
        let mut rgba = vec![0u8; SIZE * SIZE * 4];

        for y in 0..SIZE {
            for x in 0..SIZE {
                let mut covered = 0;
                for sample_y in 0..SAMPLES {
                    for sample_x in 0..SAMPLES {
                        let px = x as f32 + (sample_x as f32 + 0.5) / SAMPLES as f32;
                        let py = y as f32 + (sample_y as f32 + 0.5) / SAMPLES as f32;
                        let frame = rounded_rect(px, py, 3.0, 3.0, 33.0, 33.0, 4.0)
                            && !rounded_rect(px, py, 6.0, 6.0, 30.0, 30.0, 1.5);
                        let bars = [9.0, 16.0, 23.0]
                            .iter()
                            .any(|left| rounded_rect(px, py, *left, 10.0, *left + 4.0, 26.0, 1.0));
                        covered += usize::from(frame || bars);
                    }
                }

                let offset = (y * SIZE + x) * 4;
                rgba[offset + 3] = (covered * 255 / (SAMPLES * SAMPLES)) as u8;
            }
        }

        Ok(Icon::from_rgba(rgba, SIZE as u32, SIZE as u32)?)
    }

    #[cfg(target_os = "macos")]
    fn rounded_rect(
        x: f32,
        y: f32,
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
        radius: f32,
    ) -> bool {
        let nearest_x = x.clamp(left + radius, right - radius);
        let nearest_y = y.clamp(top + radius, bottom - radius);
        let dx = x - nearest_x;
        let dy = y - nearest_y;
        x >= left && x <= right && y >= top && y <= bottom && dx * dx + dy * dy <= radius * radius
    }

    #[cfg(target_os = "windows")]
    fn agent_icon() -> Result<Icon> {
        const SIZE: usize = 32;
        let mut rgba = vec![0u8; SIZE * SIZE * 4];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let offset = (y * SIZE + x) * 4;
                let border = x < 2 || x >= SIZE - 2 || y < 2 || y >= SIZE - 2;
                let bar = (7..=11).contains(&x) || (14..=18).contains(&x) || (21..=25).contains(&x);
                let lit = !border && (7..=24).contains(&y) && bar;
                let color = if lit {
                    [215, 239, 67, 255]
                } else {
                    [23, 28, 26, 255]
                };
                rgba[offset..offset + 4].copy_from_slice(&color);
            }
        }
        Ok(Icon::from_rgba(rgba, SIZE as u32, SIZE as u32)?)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use platform::{AgentTray, TrayAction};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub struct AgentTray;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl AgentTray {
    pub fn new(_launch_url: String, _paused: bool) -> Result<Self> {
        Ok(Self)
    }
}
