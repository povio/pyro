#!/bin/sh
set -eu
cd "$(dirname "$0")"
ffmpeg -hide_banner -loglevel error -f lavfi -i color=c=red:size=64x48:rate=1 -frames:v 1 -pix_fmt yuv420p -c:v libx264 -x264-params colorprim=smpte170m:transfer=smpte170m:colormatrix=smpte170m -profile:v baseline -level:v 3.0 -colorspace smpte170m -color_primaries smpte170m -color_trc smpte170m -color_range tv -f h264 -y smpte170m-red.h264
ffprobe -v error -show_entries stream=codec_name,profile,level,width,height,color_space,color_transfer,color_primaries,color_range -of json smpte170m-red.h264 > smpte170m-stream.json
