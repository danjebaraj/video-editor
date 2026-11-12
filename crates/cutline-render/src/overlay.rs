//! Overlay pass on the joined main video: b-roll, images, alpha video, HTML renders, ASS
//! text/captions and the progress bar.

use crate::job::Job;
use crate::{Geometry, PlanExtras};
use anyhow::Result;
use cutline_core::model::Project;
use cutline_core::timeline::Timeline;
use cutline_media::assets::ProjectMedia;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct OverlayOut {
    pub input_args: Vec<String>,
    pub scripts: Vec<(PathBuf, String)>,
    pub pre_jobs: Vec<Job>,
}

#[allow(clippy::too_many_arguments)]
pub fn apply(
    _tl: &Timeline,
    _g: &Geometry,
    _full: &Geometry,
    _lines: &mut Vec<String>,
    _cur: &mut String,
    _next_input: &mut usize,
    _extras: &PlanExtras,
    _p: &Project,
    _media: &ProjectMedia,
    _work: &Path,
) -> Result<OverlayOut> {
    Ok(OverlayOut::default())
}
