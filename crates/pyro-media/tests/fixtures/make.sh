#!/bin/bash
# Regenerates pyro-media's test fixtures; run from this directory. Needs an
# ffmpeg with libx264, libsvtav1, libmp3lame and libopus (Vorbis uses
# ffmpeg's own encoder).
set -e
F="ffmpeg -v error -y -fflags +bitexact"
A44=(-f lavfi -i "sine=frequency=440:sample_rate=44100:duration=0.5")
A48=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.5")
M=(-map_metadata -1 -ac 1)
$F "${A44[@]}" "${M[@]}" -c:a pcm_s16le sine.wav
$F "${A44[@]}" "${M[@]}" -c:a libmp3lame -b:a 48k sine.mp3
$F "${A44[@]}" "${M[@]}" -c:a aac -b:a 48k sine.m4a
$F "${A44[@]}" "${M[@]}" -c:a vorbis -strict -2 -ac 2 sine.ogg
$F "${A48[@]}" "${M[@]}" -c:a libopus -b:a 24k sine.opus
$F "${A48[@]}" "${M[@]}" -c:a libopus -b:a 24k sine.webm
$F "${A44[@]}" "${M[@]}" -c:a flac sine.flac
# Ten 64x48 frames at 10 fps; frame N is the colour rgb(N*25, 255-N*25, 128).
V=(-f lavfi -i "color=black:s=64x48:r=10:d=1,format=rgb24,geq=r='N*25':g='255-N*25':b=128")
TAGS=(-map_metadata -1 -pix_fmt yuv420p -colorspace bt709 -color_primaries bt709 -color_trc bt709)
# H.264 High with B-frames (decode order differs from presentation order).
$F "${V[@]}" "${TAGS[@]}" -c:v libx264 -profile:v high -x264-params bframes=2:b-adapt=0:keyint=5:min-keyint=5:scenecut=0 -movflags +faststart frames-h264.mp4
$F "${V[@]}" "${TAGS[@]}" -c:v libsvtav1 -g 5 frames-av1.mp4
$F "${V[@]}" "${TAGS[@]}" -c:v libsvtav1 -g 5 frames-av1.webm
