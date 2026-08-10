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
