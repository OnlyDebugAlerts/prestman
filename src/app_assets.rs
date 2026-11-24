use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString, assets};

assets::icon_assets!(
    ExtraIcons,
    [
        ListFilter,
        Code,
        Library,
        Copy,
        Lock,
        LockOpen,
        Cloud,
        CloudUpload,
        Layers,
        Server,
        User,
        UserPlus
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == "prestman-logo.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/prestman-logo.png"
            ))));
        }
        if let Some(icon) = ExtraIcons.load(path)? {
            return Ok(Some(icon));
        }
        assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        if "prestman-logo.png".starts_with(path) {
            paths.push("prestman-logo.png".into());
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_variable_icon_and_default_component_icons() -> anyhow::Result<()> {
        for name in [
            assets::IconName::ListFilter,
            assets::IconName::Code,
            assets::IconName::Library,
            assets::IconName::Copy,
            assets::IconName::Lock,
            assets::IconName::LockOpen,
            assets::IconName::Cloud,
            assets::IconName::CloudUpload,
            assets::IconName::Layers,
            assets::IconName::Server,
            assets::IconName::User,
            assets::IconName::UserPlus,
            assets::IconName::ArrowLeft,
            assets::IconName::RefreshCw,
            assets::IconName::SquareTerminal,
            assets::IconName::CircleCheck,
            assets::IconName::Settings2,
            assets::IconName::Check,
            assets::IconName::Close,
        ] {
            assert!(AppAssets.load(&name.path())?.is_some());
        }
        assert!(AppAssets.load("prestman-logo.png")?.is_some());
        Ok(())
    }
}
