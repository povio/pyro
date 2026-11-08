//! Media decoding for lightningpanda: audio for Web Audio's
//! `decodeAudioData` ([`audio`]) and video frames for `<video>` painting,
//! `drawImage` and `requestVideoFrameCallback` ([`video`]).
//!
//! Nothing here runs until a page asks: no decoder is made, no table is
//! built and no thread is started before the first decode, so a page without
//! media pays nothing.
//!
//! - Audio: Symphonia (pure Rust, MPL-2.0) demuxes WAV, MP3, MP4/M4A (AAC,
//!   ALAC, Opus, FLAC), Ogg (Vorbis, Opus, FLAC), WebM/Matroska, FLAC, AIFF
//!   and CAF, and decodes everything but Opus, which `opus-pure` (pure
//!   Rust, BSD-3-Clause) decodes.
//! - Video: an MP4 / WebM demuxer of our own ([`video`]) feeds AV1 to rav1d
//!   (pure Rust port of dav1d, BSD-2-Clause; cargo feature `av1`) and H.264
//!   to OpenH264 (Cisco's C++ decoder built from source, BSD-2-Clause;
//!   cargo feature `h264`). VP8, VP9 and HEVC have no decoder (see
//!   docs/webapi/MEDIA.md).

pub mod audio;
pub mod video;
