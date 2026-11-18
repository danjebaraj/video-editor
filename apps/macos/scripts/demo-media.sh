#!/bin/zsh
# Makes the sample media for the user guide's screenshots: two short clips of a made-up diagram
# editor, a logo and a music bed. Usage: scripts/demo-media.sh <folder>
# Needs an ffmpeg with drawtext: cutline's own (`cutline deps install ffmpeg`) has it.
setopt errexit pipefail nounset
out="$1"
mkdir -p "$out"
ffmpeg_bin=$(cutline deps status 2>/dev/null | sed -nE 's/^active ffmpeg: .*\(ToolsDir\) (.*)$/\1/p' | head -1)
[[ -x $ffmpeg_bin ]] || ffmpeg_bin=$(command -v ffmpeg)
FB="/System/Library/Fonts/Supplemental/Arial Bold.ttf"
FR="/System/Library/Fonts/Helvetica.ttc"
run() { "$ffmpeg_bin" -y -hide_banner -loglevel error "$@"; }

# The editor, with a slowly moving gradient and a cursor so QA doesn't see a frozen picture.
run -f lavfi -i "color=c=0xF4F6FA:s=1920x1080:r=30:d=14" \
    -f lavfi -i "gradients=s=1920x1080:r=30:d=14:speed=0.04:c0=0xF26B21:c1=0x2D9CDB:c2=0x1F2A44:nb_colors=3" \
    -filter_complex "[1]format=yuv420p[g];[0][g]blend=all_mode=normal:all_opacity=0.10,
drawbox=x=0:y=0:w=1920:h=96:color=0x1F2A44:t=fill,
drawbox=x=0:y=96:w=380:h=984:color=0xE6EAF3:t=fill,
drawbox=x=40:y=150:w=300:h=70:color=0xFFFFFF:t=fill,
drawbox=x=40:y=240:w=300:h=70:color=0xFFFFFF:t=fill,
drawbox=x=40:y=330:w=300:h=70:color=0xFFFFFF:t=fill,
drawbox=x=40:y=420:w=300:h=70:color=0xFFFFFF:t=fill,
drawbox=x=560:y=330:w=300:h=140:color=0x2D9CDB:t=fill,
drawbox=x=1000:y=330:w=300:h=140:color=0xF26B21:t=fill,
drawbox=x=1440:y=330:w=300:h=140:color=0x1F2A44:t=fill,
drawbox=x=860:y=396:w=140:h=8:color=0x6B7280:t=fill,
drawbox=x=1300:y=396:w=140:h=8:color=0x6B7280:t=fill,
drawbox=x=1000:y=470:w=8:h=160:color=0x6B7280:t=fill,
drawbox=x=1000:y=630:w=300:h=140:color=0x16A34A:t=fill,
drawtext=fontfile='$FB':text='Diagram editor':x=40:y=26:fontsize=44:fontcolor=white,
drawtext=fontfile='$FR':text='Shapes':x=40:y=108:fontsize=28:fontcolor=0x6B7280,
drawtext=fontfile='$FR':text='Start':x=70:y=165:fontsize=34:fontcolor=0x1F2A44,
drawtext=fontfile='$FR':text='Process':x=70:y=255:fontsize=34:fontcolor=0x1F2A44,
drawtext=fontfile='$FR':text='Decision':x=70:y=345:fontsize=34:fontcolor=0x1F2A44,
drawtext=fontfile='$FR':text='End':x=70:y=435:fontsize=34:fontcolor=0x1F2A44,
drawtext=fontfile='$FB':text='Start':x=660:y=380:fontsize=44:fontcolor=white,
drawtext=fontfile='$FB':text='Review':x=1090:y=380:fontsize=44:fontcolor=white,
drawtext=fontfile='$FB':text='Publish':x=1530:y=380:fontsize=44:fontcolor=white,
drawtext=fontfile='$FB':text='Done':x=1100:y=680:fontsize=44:fontcolor=white,
drawbox=x='420+mod(t*110\,1300)':y='840+30*sin(t*3)':w=44:h=44:color=0xF26B21:t=fill,format=yuv420p" \
    -c:v libx264 -pix_fmt yuv420p -crf 20 "$out/diagram-demo.mov"

# A close-up of the shape palette.
run -f lavfi -i "color=c=0x1F2A44:s=1920x1080:r=30:d=10" \
    -f lavfi -i "gradients=s=1920x1080:r=30:d=10:speed=0.05:c0=0xF26B21:c1=0x2D9CDB:nb_colors=2" \
    -filter_complex "[1]format=yuv420p[g];[0][g]blend=all_mode=normal:all_opacity=0.18,
drawbox=x=160:y=160:w=520:h=760:color=0xFFFFFF:t=fill,
drawbox=x=200:y=210:w=440:h=90:color=0xF26B21:t=fill,
drawbox=x=200:y=330:w=440:h=90:color=0x2D9CDB:t=fill,
drawbox=x=200:y=450:w=440:h=90:color=0x16A34A:t=fill,
drawbox=x=200:y=570:w=440:h=90:color=0xE6EAF3:t=fill,
drawtext=fontfile='$FB':text='Shape palette':x=760:y=420:fontsize=96:fontcolor=white,
drawtext=fontfile='$FR':text='Drag, drop, connect':x=764:y=560:fontsize=52:fontcolor=0xF26B21,format=yuv420p" \
    -c:v libx264 -pix_fmt yuv420p -crf 20 "$out/palette-closeup.mov"

run -f lavfi -i "color=c=0x1F2A44:s=512x512:d=1,format=rgb24" \
    -vf "drawbox=x=96:y=96:w=320:h=320:color=0xF26B21:t=fill,drawtext=fontfile='$FB':text='A':x=(w-text_w)/2:y=(h-text_h)/2-8:fontsize=240:fontcolor=white" \
    -frames:v 1 "$out/logo-dark.png"

run -f lavfi -i "sine=f=196:d=30" -f lavfi -i "sine=f=294:d=30" -f lavfi -i "sine=f=392:d=30" \
    -filter_complex "[0][1][2]amix=inputs=3,volume=0.5,tremolo=f=2:d=0.3" -c:a aac -b:a 128k "$out/brand-theme.m4a"
print "✔ sample media in $out"
