//! BrawlBuilderEx build engine: the classic BrawlBuilder pipeline (wit extract / copy mod files / GCT patch / build) and the EX build,
//! which turns launcher-based mods (Project+, P+Ex, Legacy, Minus, Void, ...) into self-contained disc images.

pub mod bin;
pub mod cli;
pub mod dol;
pub mod exbuild;
pub mod exmod;
pub mod fat;
pub mod fsutil;
pub mod gecko;
pub mod lz;
pub mod pipeline;
pub mod ppc;
pub mod sdpack;
pub mod shims;
pub mod ssemenu;
pub mod thp;
pub mod wit;

pub use bin::{ExError, Result};
