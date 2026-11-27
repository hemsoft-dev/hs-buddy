//! Asset source: the default component icons plus the Lucide icons Buddy uses.
//!
//! Only the icons listed here are embedded in the binary; see the gpui-kit
//! assets README for why the full catalog is not registered.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(
    BuddyIcons,
    [
        Activity,
        ArrowRight,
        Bookmark,
        Bot,
        Building2,
        Calendar,
        ChartColumn,
        ChevronDown,
        ChevronUp,
        Clock,
        Cloud,
        CloudFog,
        CloudLightning,
        CloudRain,
        CloudSnow,
        DollarSign,
        Droplets,
        Eye,
        EyeOff,
        Flower2,
        FolderGit2,
        Github,
        Handshake,
        GitPullRequest,
        Heart,
        LayoutDashboard,
        LayoutGrid,
        Leaf,
        List,
        MapPin,
        PartyPopper,
        PanelBottom,
        Play,
        Plus,
        RefreshCw,
        Search,
        Settings,
        Sparkles,
        Sprout,
        SquareCheck,
        SquareTerminal,
        Star,
        Sun,
        Thermometer,
        TreePine,
        Users,
        Wind,
        X,
        Zap
    ]
);

/// Buddy's icons first, then the component defaults.
pub struct BuddyAssets;

impl AssetSource for BuddyAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = BuddyIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(BuddyIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
