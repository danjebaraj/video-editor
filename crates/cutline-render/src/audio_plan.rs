//! Audio stage: stems → chains → ducking → mix → loudness → final WAV.

use crate::job::{base_args, Job};
use crate::{assemble, Geometry, PlanExtras, RenderOptions};
use anyhow::Result;
use cutline_core::model::Project;
use cutline_core::timeline::Timeline;
use cutline_deps::Toolchain;
use cutline_media::assets::ProjectMedia;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct AudioPlan {
    pub mix_job: Job,
    pub mix_wav: PathBuf,
    pub final_wav: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoudnessResult {
    pub input_i: f64,
    pub input_tp: f64,
    pub output_i: f64,
    pub output_tp: f64,
}

#[allow(clippy::too_many_arguments)]
pub fn plan(
    tc: &Toolchain,
    _p: &Project,
    tl: &Timeline,
    g: &Geometry,
    mezz: &[PathBuf],
    _media: &ProjectMedia,
    work: &Path,
    _dir: &Path,
    _extras: &PlanExtras,
    _opts: &RenderOptions,
) -> Result<AudioPlan> {
    let mut j = Job::new("audio mix");
    j.args(base_args(false));
    for m in mezz {
        j.args(["-i".to_string(), m.to_string_lossy().to_string()]);
    }
    let (mut lines, out) = assemble::join_dialogue(tl, g, 0);
    lines.push(format!("[{out}]anull[mix]"));
    let script = work.join("audio_mix.filter");
    j.args(tc.filter_complex_file_args(&script));
    let wav = work.join("mix.wav");
    j.args(["-map", "[mix]", "-c:a", "pcm_f32le", "-ar", "48000", "-ac", "2"]);
    j.arg(wav.to_string_lossy().to_string());
    j.scripts.push((script, lines.join(";\n") + "\n"));
    j.outputs.push(wav.clone());
    Ok(AudioPlan { mix_job: j, mix_wav: wav.clone(), final_wav: wav })
}

pub fn execute(tc: &Toolchain, a: &AudioPlan) -> Result<Option<LoudnessResult>> {
    a.mix_job.run(tc)?;
    Ok(None)
}

pub fn jobs_for_display(a: &AudioPlan) -> Vec<Job> {
    vec![a.mix_job.clone()]
}
