<#
Optional live test on Windows (ui/spec.md §23.9 item 6): one real end-to-end run with Claude Haiku
and a 10-second Shorts output. It uses your signed-in Claude Code, Git Bash and the real cutline,
and costs a few cents. Nothing is written outside the scratch folder it prints.

  scripts\video-studio-live-test.ps1 [-Scratch <folder>] [-Cli <path to video-studio.exe>]

The CLI is, in order: -Cli; the installed MSIX's execution alias (video-studio.exe); the debug
build in target\debug (built when missing). After the run it checks the output file, its QA JSON
and the run's logs, then runs `video-studio run --json` on the same project and checks the events.
Exits non-zero when a check fails.
#>
param(
  [string]$Scratch = (Join-Path ([IO.Path]::GetTempPath()) "video-studio-live-$(Get-Date -Format yyyyMMdd-HHmmss)"),
  [string]$Cli
)
$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..')
$failures = 0
function Check([bool]$ok, [string]$what) {
  if ($ok) { Write-Host "ok   $what" } else { Write-Host "FAIL $what" -ForegroundColor Red; $script:failures++ }
}
function Native([string]$exe, [string[]]$argv) {
  $ErrorActionPreference = 'Continue'
  # the command's own output goes to the console, so the function returns only the exit code
  & $exe @argv | Out-Host
  return $LASTEXITCODE
}

# The CLI under test
if (-not $Cli) {
  $pkg = Get-AppxPackage -Name 'Syncfusion.VideoStudio' -ErrorAction SilentlyContinue
  $alias = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\video-studio.exe'
  if ($pkg -and (Test-Path $alias)) {
    $Cli = $alias
    Write-Host "Using the installed MSIX $($pkg.Version) ($Cli)"
  } else {
    Push-Location $repo
    try { Native cargo @('build', '-q', '-p', 'video-studio-cli', '-p', 'cutline-cli') | Out-Null } finally { Pop-Location }
    $Cli = Join-Path $repo 'target\debug\video-studio.exe'
    Write-Host "No installed MSIX: using the debug build ($Cli)"
  }
}

# The real cutline (not the test double): PATH, else the workspace build
$cutline = (Get-Command cutline -ErrorAction SilentlyContinue).Source
if (-not $cutline) { $cutline = Join-Path $repo 'target\debug\cutline.exe' }
if (-not (Test-Path $cutline)) { throw "cutline wasn't found: put it on PATH or run cargo build -p cutline-cli" }
$env:VIDEO_STUDIO_CUTLINE = $cutline
$ffmpeg = (& $cutline deps path ffmpeg).Trim()
if (-not (Test-Path $ffmpeg)) { throw "ffmpeg wasn't found: run cutline deps install ffmpeg" }

New-Item -ItemType Directory -Force (Join-Path $Scratch 'footage') | Out-Null
Write-Host "Scratch folder: $Scratch"
$clip = Join-Path $Scratch 'footage\demo-clip.mp4'
Native $ffmpeg @('-y', '-loglevel', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=1920x1080:rate=30:duration=12',
  '-f', 'lavfi', '-i', 'sine=frequency=330:duration=12', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', $clip) | Out-Null
Check (Test-Path $clip) 'test footage written'

$env:VIDEO_STUDIO_HOME = Join-Path $Scratch 'library'
Push-Location $Scratch
try {
  $code = Native $Cli @('new', 'live.cutproj', '--intent', 'social_clip', '--name', 'Live test', '--media', 'footage')
  Check ($code -eq 0) 'video-studio new'

  $p = Get-Content 'live.cutproj' -Raw -Encoding UTF8 | ConvertFrom-Json
  $p.spec = @'
# Live test clip

A 10-second vertical clip made from one piece of footage. No narration and no music: keep the clip's own audio.

## Scene 1: The clip
- **Duration:** 10 s
- **Media:** ![demo clip](media:m-demo-clip)
- **On-screen text:** Hello, Video Studio

Show the footage for the full 10 seconds with the on-screen text as a title for the first 3 seconds.

## Call to action
None: this is a technical test.
'@
  $p.intent | Add-Member -Force target_length_s 10
  $p.output_configs = @([pscustomobject]@{ id = 'out_shorts'; preset = 'youtube_shorts'; overrides = [pscustomobject]@{} })
  $rc = $p.run_configs[0]
  $rc.name = 'Shorts'
  $rc.outputs = @('out_shorts')
  $rc.production_model = 'haiku'
  $rc | Add-Member -Force quality 'draft'
  $rc | Add-Member -Force output_folder (Join-Path $Scratch 'out')
  $rc | Add-Member -Force dry_run_first $true
  [IO.File]::WriteAllText((Join-Path $Scratch 'live.cutproj'), ($p | ConvertTo-Json -Depth 40))

  # 1. The run, with progress lines
  $code = Native $Cli @('run', 'live.cutproj', '--config', 'Shorts', '--yes', '--log-level', 'debug')
  Check ($code -le 1) "video-studio run finished (exit $code; 1 is success with warnings)"

  $runs = @(Get-ChildItem (Join-Path $Scratch 'out') -Directory -ErrorAction SilentlyContinue | Sort-Object LastWriteTime)
  $run = $runs | Where-Object { Test-Path (Join-Path $_.FullName 'outputs') } | Select-Object -Last 1
  Check ($null -ne $run) 'a run folder was written'
  if ($run) {
    Write-Host "Run folder: $($run.FullName)"
    $mp4 = Get-ChildItem (Join-Path $run.FullName 'outputs') -Recurse -Filter *.mp4 | Select-Object -First 1
    Check ($null -ne $mp4) 'the Shorts output file exists'
    if ($mp4) {
      $ffprobe = Join-Path (Split-Path $ffmpeg) 'ffprobe.exe'
      if (Test-Path $ffprobe) {
        $info = (& $ffprobe -v error -show_entries 'format=duration:stream=width,height' -of json $mp4.FullName) | ConvertFrom-Json
        $v = $info.streams | Where-Object { $_.width } | Select-Object -First 1
        $dur = [double]$info.format.duration
        Check ($v.height -gt $v.width) "the output is vertical ($($v.width)x$($v.height))"
        Check ($dur -gt 8 -and $dur -lt 13) "the output lasts about 10 s ($([Math]::Round($dur, 1)) s)"
      }
      $qa = Join-Path $mp4.DirectoryName ($mp4.BaseName + '.qa.json')
      Check (Test-Path $qa) 'the QA JSON exists next to the output'
      if (Test-Path $qa) {
        $q = Get-Content $qa -Raw | ConvertFrom-Json
        Write-Host "     QA: ok=$($q.ok) errors=$($q.errors) warnings=$($q.warnings)"
        if ($q.errors -gt 0) { Write-Host "     note: QA found $($q.errors) error(s); see the QA JSON and summary.md" -ForegroundColor Yellow }
      }
    }
    $logs = Join-Path $run.FullName 'logs'
    foreach ($f in 'run.json', 'summary.md', 'events.ndjson', 'claude.jsonl') {
      $hit = Get-ChildItem $logs -Recurse -Filter $f -ErrorAction SilentlyContinue | Select-Object -First 1
      Check ($null -ne $hit -and $hit.Length -gt 0) "logs\$f is present"
    }
    $status = (Get-Content (Join-Path $logs 'run.json') -Raw -ErrorAction SilentlyContinue | ConvertFrom-Json).status
    Check ("$status" -match '^succeeded(_with_warnings)?$') "run.json status is a success ($status)"
  }

  # 2. The same project through `run --json` (NDJSON events), skipping the dry run this time
  $events = Join-Path $Scratch 'run-json.ndjson'
  $ErrorActionPreference = 'Continue'
  & $Cli run live.cutproj --config Shorts --yes --no-dry-run --json > $events
  $code = $LASTEXITCODE
  $ErrorActionPreference = 'Stop'
  Check ($code -le 1) "video-studio run --json finished (exit $code)"
  $lines = @(Get-Content $events -Encoding UTF8 | Where-Object { $_.Trim() })
  $parsed = @($lines | ForEach-Object { try { $_ | ConvertFrom-Json } catch { $null } } | Where-Object { $_ })
  Check ($lines.Count -gt 0 -and $parsed.Count -eq $lines.Count) "every stdout line is a JSON event ($($lines.Count) lines)"
  $finished = $parsed | Where-Object { $_.type -eq 'run_finished' -or $_.event -eq 'run_finished' }
  Check ($null -ne $finished) 'the events end with run_finished'
} catch {
  Write-Host "FAIL $($_.Exception.Message) ($($_.InvocationInfo.PositionMessage))" -ForegroundColor Red
  $failures++
} finally {
  Pop-Location
}

if ($failures) { Write-Host "`n$failures check(s) failed. Scratch: $Scratch" -ForegroundColor Red; exit 1 }
Write-Host "`nLive test passed. Scratch: $Scratch"
