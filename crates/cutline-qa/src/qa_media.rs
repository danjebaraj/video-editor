//! Media-content QA checks (loudness, clipping, silence, black, freeze, first frame, hook,
//! captions). Filled in by M2/M7.

use crate::qa::{Builder, QaContext};
use anyhow::Result;
use cutline_core::target::TargetSpec;
use cutline_deps::Toolchain;
use std::path::Path;

pub fn media_checks(_tc: &Toolchain, _file: &Path, _t: &TargetSpec, _ctx: &QaContext, _b: &mut Builder) -> Result<()> {
    Ok(())
}
