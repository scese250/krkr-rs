//! FFmpeg audio decode (Linux/desktop) — the desktop build's first choice
//! for a track, ahead of the built-in decoder.
//!
//! The built-in decoder is symphonia, which has no SBR synthesis: an AAC
//! track with the SBR extension (HE-AAC) is decoded at the core's rate while
//! the container declares the extension's rate, so the track would play at the
//! wrong speed. FFmpeg's AAC decoder reconstructs the SBR band, so this module
//! decodes such a track at the rate the container declares.
//!
//! # Two shapes
//!
//! The backend offers the same two shapes as the built-in decoder:
//!
//! * [`decode_audio_ffmpeg`] materializes a whole track into interleaved
//!   `f32`.
//! * [`open_audio_ffmpeg`] + [`read_audio_ffmpeg`] + [`seek_audio_ffmpeg`]
//!   stream from an [`FfmpegAudioState`] that the middle layer owns and
//!   passes back on every call.
//!
//! The streaming shape is the one a game plays: the open reads the container
//! and its header and decodes nothing else, and the packets are pulled as the
//! playback reads them, so a track is never held in memory as a whole.
//!
//! # The container's priming
//!
//! An AAC track is encoded with a delay: the first frames hold the encoder's
//! warm-up, not the recording, and the container says how many samples they
//! are — the track's `initial_padding`, which the demuxer hands to the
//! decoder as the skip-samples side data of the first packet. The decoder
//! removes them, so the track this module returns starts at the recording
//! and ends at the recording's end; without that the voice would start a
//! frame late and run long.
//!
//! # The channel layout
//!
//! The track's channel count is the one the container declares. A track whose
//! `AudioSpecificConfig` declares a mono core stays mono even when its
//! bitstream carries the parametric-stereo extension: the extension is only
//! reconstructed for a track the container declares as stereo.

use std::fmt;
use std::io::Cursor;
use std::sync::Arc;

use api::error::MediaError;
use api::media::{AudioSpec, DecodedAudio};
use ffmpeg::ChannelLayout;
use ffmpeg::Error as FfmpegError;
use ffmpeg::Packet;
use ffmpeg::format::Sample;
use ffmpeg::format::context::Input;
use ffmpeg::format::context::StreamIo;
use ffmpeg::format::sample;
use ffmpeg::media::Type as MediaType;
use ffmpeg::software::resampling;
use ffmpeg::util::frame::audio::Audio as AudioFrame;
use ffmpeg_next as ffmpeg;

use crate::Media;
use crate::hint::{display_name, extension_of};

/// The state of one streaming FFmpeg audio decode.
///
/// The demuxer, the decoder and the converter live in this value, which the
/// middle layer owns and passes back on every call; `Media` stores nothing
/// here. The compressed bytes are retained so a seek can reopen the
/// container.
///
/// The FFmpeg contexts are not shareable on their own, so the value is
/// `Send` and not `Sync`: a caller that shares it across threads (the
/// middle layer's worker) keeps it behind a lock.
#[non_exhaustive]
pub struct FfmpegAudioState {
    /// The demuxer, the decoder and the samples they produced.
    inner: FfmpegAudioInner,
    /// The compressed bytes, retained for the reseek.
    bytes: Arc<[u8]>,
    /// The original hint, retained for the reseek.
    hint: Option<String>,
    /// The name the errors report.
    name: String,
}

impl fmt::Debug for FfmpegAudioState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FfmpegAudioState")
            .field("name", &self.name)
            .field("spec", &self.inner.track.spec)
            .field("position_frames", &self.inner.position_frames)
            .field("eof", &self.inner.eof)
            .finish_non_exhaustive()
    }
}

/// The demuxer, the decoder and the samples one open track produced.
struct FfmpegAudioInner {
    /// The demuxer over the container's bytes.
    input: Input,
    /// The track's decoder and converter.
    track: TrackDecoder,
    /// The decoded-but-not-yet-returned interleaved samples.
    pending: Vec<f32>,
    /// The consumed offset into `pending`.
    pending_offset: usize,
    /// The frames returned to the caller so far (the decode position).
    position_frames: u64,
    /// Set once the track is exhausted.
    eof: bool,
}

/// One audio track's decoder and the converter to the mixer's format.
///
/// The whole-file path and the streaming path share it, so the samples are
/// the same whichever path decodes them.
///
/// The state holds no channel layout: a layout carries a pointer for a
/// custom layout, so it does not travel between threads. The two layouts the
/// decode needs are read from the decoder and from the converter where they
/// are used.
struct TrackDecoder {
    /// The FFmpeg audio decoder.
    decoder: ffmpeg::decoder::Audio,
    /// The stream the decoder reads.
    stream_index: usize,
    /// The converter to the mixer's format.
    resampler: Option<resampling::Context>,
    /// The input parameters the converter was built from (the sample format,
    /// the channel layout mask and the rate), so a frame whose format,
    /// layout or rate differs gets its own converter.
    resampler_input: Option<(Sample, u64, u32)>,
    /// The format the decoder produces.
    spec: AudioSpec,
    /// Set once the demuxer has reached its end.
    demux_eof: bool,
    /// Set once the converter has been flushed.
    flushed: bool,
}

/// Decodes a whole audio track with FFmpeg into interleaved `f32` samples.
///
/// The result carries the decoded [`AudioSpec`]; the samples are interleaved
/// per channel (`interleaved[frame * channels + channel]`). The whole track
/// is materialized, so the caller owns the samples and no decoding happens
/// during playback.
///
/// `hint` is the storage name or extension that located the bytes;
/// it only steers the container detection and is never trusted over the
/// content.
///
/// # Errors
///
/// [`MediaError::Unsupported`] when the container holds no audio stream,
/// [`MediaError::Decode`] when the container or the stream is malformed, and
/// [`MediaError::Io`] when the bytes cannot be demuxed. No silence is
/// substituted for a failed decode.
pub fn decode_audio_ffmpeg(
    m: &Media,
    bytes: Arc<[u8]>,
    hint: Option<&str>,
) -> Result<Arc<DecodedAudio>, MediaError> {
    let _ = m;
    let name = display_name(hint, "<audio>");
    let mut input = open_input(&bytes, hint, name)?;
    let stream_index = audio_stream_index(&input, name)?;
    let track = decode_audio_track(&mut input, stream_index, name)?;
    Ok(Arc::new(track))
}

/// Opens an audio stream for incremental decoding.
///
/// On success returns the stream's [`AudioSpec`] plus a fresh
/// [`FfmpegAudioState`] for [`read_audio_ffmpeg`]/[`seek_audio_ffmpeg`].
/// `bytes` is shared by `Arc` so the caller can keep the compressed buffer
/// alive (and so a seek can reopen the container without re-copying it).
///
/// The container is opened and its header read here; the samples are pulled
/// as the playback reads them, so the open never decodes the track. The
/// first frame is decoded to learn the format the decoder really produces,
/// so the returned spec is the format of the samples.
///
/// The frame count is the one the container declares, so the track's length
/// is known without decoding it.
///
/// # Errors
///
/// [`MediaError::Unsupported`] when the container holds no audio stream,
/// [`MediaError::Decode`] when the container is malformed or yields no
/// decodable audio, and [`MediaError::Io`] when the bytes cannot be demuxed.
/// No silence is substituted for a failed decode.
pub fn open_audio_ffmpeg(
    m: &Media,
    bytes: Arc<[u8]>,
    hint: Option<&str>,
) -> Result<(AudioSpec, FfmpegAudioState), MediaError> {
    let _ = m;
    let name = display_name(hint, "<audio>");
    let mut input = open_input(&bytes, hint, name)?;
    let stream_index = audio_stream_index(&input, name)?;
    let mut track = TrackDecoder::open(&input, stream_index, name)?;
    // The first frame is decoded here, so the spec is the format the decoder
    // really produces and the track's first samples are staged.
    let mut pending: Vec<f32> = Vec::new();
    if !track.decode_next(&mut input, &mut pending, name)? {
        return Err(MediaError::Decode {
            format: String::new(),
            message: format!("{name}: no audio data decoded"),
        });
    }
    let mut spec = track.spec;
    spec.total_frames = container_frames(&input, stream_index, spec.sample_rate);
    track.spec = spec;
    Ok((
        spec,
        FfmpegAudioState {
            inner: FfmpegAudioInner {
                input,
                track,
                pending,
                pending_offset: 0,
                position_frames: 0,
                eof: false,
            },
            bytes,
            hint: hint.map(str::to_string),
            name: name.to_string(),
        },
    ))
}

/// Decodes the next chunk of interleaved `f32` samples into `out`.
///
/// Returns the number of samples written, always a whole number of frames
/// (the tail of `out` that cannot hold a complete frame is left untouched).
///
/// # Errors
///
/// [`MediaError::EndOfStream`] marks the end of the track,
/// [`MediaError::InvalidData`] when the channel count cannot be represented
/// in memory, and [`MediaError::Decode`] when the decoder fails on the next
/// packet.
pub fn read_audio_ffmpeg(
    m: &Media,
    state: &mut FfmpegAudioState,
    out: &mut [f32],
) -> Result<usize, MediaError> {
    let _ = m;
    let inner = &mut state.inner;
    let channels =
        usize::try_from(inner.track.spec.channels.max(1)).map_err(|_| MediaError::InvalidData {
            message: format!(
                "audio channel count {} does not fit in memory",
                inner.track.spec.channels
            ),
        })?;
    let frame_aligned = out.len() - (out.len() % channels);
    let mut written = 0usize;

    while written < frame_aligned {
        // Drain the staged samples before touching the demuxer.
        if inner.pending_offset < inner.pending.len() {
            let available = inner.pending.len() - inner.pending_offset;
            let take = available.min(frame_aligned - written);
            out[written..written + take]
                .copy_from_slice(&inner.pending[inner.pending_offset..inner.pending_offset + take]);
            inner.pending_offset += take;
            written += take;
            if inner.pending_offset == inner.pending.len() {
                inner.pending.clear();
                inner.pending_offset = 0;
            }
            continue;
        }
        if inner.eof {
            break;
        }
        if !inner
            .track
            .decode_next(&mut inner.input, &mut inner.pending, &state.name)?
        {
            inner.eof = true;
            break;
        }
    }

    inner.position_frames += (written / channels) as u64;
    if written == 0 {
        if inner.eof {
            return Err(MediaError::EndOfStream);
        }
        // `out` is smaller than one frame; report "nothing written" so the
        // caller can retry with a larger buffer.
        return Ok(0);
    }
    Ok(written)
}

/// Seeks the stream so the next [`read_audio_ffmpeg`] starts at `frame`.
///
/// The container is reopened from the retained bytes and the frames before
/// `frame` are decoded and dropped, so the samples at `frame` are the ones
/// the decoder produces from the start of the track.
///
/// # Errors
///
/// The errors of [`open_audio_ffmpeg`]: a track that cannot be reopened
/// reports the same typed failure as its first open.
pub fn seek_audio_ffmpeg(
    m: &Media,
    state: &mut FfmpegAudioState,
    frame: u64,
) -> Result<(), MediaError> {
    let _ = m;
    let name = display_name(state.hint.as_deref(), "<audio>");
    let inner = &mut state.inner;
    let mut input = open_input(&state.bytes, state.hint.as_deref(), name)?;
    let stream_index = audio_stream_index(&input, name)?;
    let mut track = TrackDecoder::open(&input, stream_index, name)?;
    let mut pending: Vec<f32> = Vec::new();
    if !track.decode_next(&mut input, &mut pending, name)? {
        return Err(MediaError::Decode {
            format: String::new(),
            message: format!("{name}: no audio data decoded"),
        });
    }
    let channels =
        usize::try_from(track.spec.channels.max(1)).map_err(|_| MediaError::InvalidData {
            message: format!(
                "audio channel count {} does not fit in memory",
                track.spec.channels
            ),
        })?;
    let mut total = (pending.len() / channels) as u64;
    // The frames before the target are decoded and dropped; the chunk that
    // reaches the target is kept, so the read that follows starts there.
    let mut ended = false;
    while total < frame {
        let mut chunk: Vec<f32> = Vec::new();
        if !track.decode_next(&mut input, &mut chunk, name)? {
            ended = true;
            break;
        }
        total += (chunk.len() / channels) as u64;
        pending = chunk;
    }
    // The staged chunk holds the frames that follow the ones decoded before
    // it; the frames of the chunk that lie before the target are dropped.
    // A track that ends before the target leaves the stream at its end.
    let skip = if ended {
        pending.len()
    } else {
        let staged = (pending.len() / channels) as u64;
        let staged_before = total.saturating_sub(staged);
        let frames = frame.saturating_sub(staged_before);
        usize::try_from(frames)
            .ok()
            .and_then(|frames| frames.checked_mul(channels))
            .ok_or_else(|| MediaError::InvalidData {
                message: "the number of frames to skip does not fit in memory".to_owned(),
            })?
    };
    if skip > pending.len() {
        return Err(MediaError::InvalidState {
            message: "the seek skipped more frames than the track holds".to_owned(),
        });
    }
    pending.drain(..skip);
    let mut spec = track.spec;
    spec.total_frames = container_frames(&input, stream_index, spec.sample_rate);
    track.spec = spec;
    inner.input = input;
    inner.track = track;
    inner.pending = pending;
    inner.pending_offset = 0;
    inner.position_frames = frame;
    inner.eof = ended;
    Ok(())
}

/// The index of the container's audio stream.
fn audio_stream_index(input: &Input, name: &str) -> Result<usize, MediaError> {
    input
        .streams()
        .best(MediaType::Audio)
        .map(|stream| stream.index())
        .ok_or_else(|| MediaError::Unsupported {
            message: format!("{name}: no audio stream"),
        })
}

/// Opens an in-memory FFmpeg demuxer over `bytes`.
///
/// The extension hint is passed to FFmpeg for format detection only; the
/// bytes always decide the container. The `Arc<[u8]>` is shared into the
/// `StreamIo`, so no whole-buffer copy is made.
pub(crate) fn open_input(
    bytes: &Arc<[u8]>,
    hint: Option<&str>,
    name: &str,
) -> Result<Input, MediaError> {
    ffmpeg::init().map_err(|error| MediaError::Io {
        message: format!("{name}: ffmpeg init failed: {error}"),
    })?;
    let probe_name = hint
        .and_then(extension_of)
        .map(|ext| format!("input.{ext}"));
    let io = StreamIo::from_read_seek(Cursor::new(Arc::clone(bytes))).map_err(|error| {
        MediaError::Io {
            message: format!("{name}: {error}"),
        }
    })?;
    ffmpeg::format::input_from_stream(io, probe_name.as_deref(), None).map_err(|error| {
        MediaError::Decode {
            format: String::new(),
            message: format!("{name}: open demuxer: {error}"),
        }
    })
}

/// Decodes every packet of the audio track `stream_index` into interleaved
/// `f32`, resampling to the decoder's native rate/layout.
pub(crate) fn decode_audio_track(
    input: &mut Input,
    stream_index: usize,
    name: &str,
) -> Result<DecodedAudio, MediaError> {
    let mut track = TrackDecoder::open(input, stream_index, name)?;
    let mut samples: Vec<f32> = Vec::new();
    let mut pending: Vec<f32> = Vec::new();
    loop {
        pending.clear();
        if !track.decode_next(input, &mut pending, name)? {
            break;
        }
        samples.extend_from_slice(&pending);
    }

    if samples.is_empty() {
        return Err(MediaError::Decode {
            format: String::new(),
            message: format!("{name}: no audio data decoded"),
        });
    }

    let spec = track.spec;
    let total_frames = if spec.channels > 0 {
        Some(samples.len() as u64 / u64::from(spec.channels))
    } else {
        None
    };
    Ok(DecodedAudio::new(
        AudioSpec {
            total_frames,
            ..spec
        },
        samples,
    ))
}

/// The frame count the container declares for the audio track.
///
/// The container's own length is the header's number: it is what the script
/// reads as the track's duration, and it is known without decoding the
/// track. A container that declares no length reports none.
fn container_frames(input: &Input, stream_index: usize, rate: u32) -> Option<u64> {
    if rate == 0 {
        return None;
    }
    if let Some(stream) = input.stream(stream_index) {
        let duration = u64::try_from(stream.duration()).ok();
        let time_base = stream.time_base();
        let denominator = u64::try_from(time_base.denominator()).ok();
        let numerator = u64::try_from(time_base.numerator()).ok();
        if let (Some(duration), Some(denominator), Some(numerator)) =
            (duration, denominator, numerator)
            && duration > 0
            && denominator > 0
        {
            let frames = u128::from(duration) * u128::from(numerator) * u128::from(rate);
            let frames = (frames + u128::from(denominator) / 2) / u128::from(denominator);
            return u64::try_from(frames).ok();
        }
    }
    // The container's own duration, in microseconds.
    let duration = u64::try_from(input.duration()).ok();
    match duration {
        Some(duration) if duration > 0 => {
            let frames = u128::from(duration) * u128::from(rate) / 1_000_000;
            u64::try_from(frames).ok()
        }
        _ => None,
    }
}

impl TrackDecoder {
    /// Opens the decoder for the audio stream `stream_index` of `input`.
    fn open(input: &Input, stream_index: usize, name: &str) -> Result<Self, MediaError> {
        let parameters = input
            .stream(stream_index)
            .map(|stream| stream.parameters())
            .ok_or_else(|| MediaError::InvalidState {
                message: format!("{name}: audio stream vanished after probing"),
            })?;
        let context =
            ffmpeg::codec::context::Context::from_parameters(parameters).map_err(|error| {
                MediaError::Unsupported {
                    message: format!("{name}: {error}"),
                }
            })?;
        let decoder = context
            .decoder()
            .audio()
            .map_err(|error| MediaError::Unsupported {
                message: format!("{name}: no audio decoder: {error}"),
            })?;
        Ok(Self {
            decoder,
            stream_index,
            resampler: None,
            resampler_input: None,
            spec: AudioSpec::default(),
            demux_eof: false,
            flushed: false,
        })
    }

    /// Decodes the next frame of the track and appends its converted samples
    /// to `pending`.
    ///
    /// The packets are pulled from the demuxer as the frames are needed, so
    /// the track is decoded as it is read. Returns `Ok(false)` at the end of
    /// the track.
    fn decode_next(
        &mut self,
        input: &mut Input,
        pending: &mut Vec<f32>,
        name: &str,
    ) -> Result<bool, MediaError> {
        loop {
            let mut frame = AudioFrame::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    let converted = self.convert(&mut frame, name)?;
                    pending.extend_from_slice(&converted);
                    return Ok(true);
                }
                Err(FfmpegError::Eof) => {
                    // The decoder is drained; the converter may still hold
                    // samples of the track.
                    if !self.flushed {
                        self.flushed = true;
                        let tail = self.flush(name)?;
                        if !tail.is_empty() {
                            pending.extend_from_slice(&tail);
                            return Ok(true);
                        }
                    }
                    return Ok(false);
                }
                Err(FfmpegError::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
                Err(error) => {
                    return Err(MediaError::Decode {
                        format: String::new(),
                        message: format!("{name}: audio decode: {error}"),
                    });
                }
            }

            if self.demux_eof {
                return Ok(false);
            }

            let mut packet = Packet::empty();
            match packet.read(input) {
                Ok(()) => {
                    if packet.stream() == self.stream_index {
                        // The packet is handed to the decoder as it came: the
                        // demuxer's skip-samples side data, which carries the
                        // container's priming, is what makes the decoder drop
                        // the encoder's delay at the start of the track.
                        self.decoder
                            .send_packet(&packet)
                            .map_err(|error| MediaError::Decode {
                                format: String::new(),
                                message: format!("{name}: audio packet: {error}"),
                            })?;
                    }
                }
                Err(FfmpegError::Eof) => {
                    self.decoder
                        .send_eof()
                        .map_err(|error| MediaError::Decode {
                            format: String::new(),
                            message: format!("{name}: audio decoder flush: {error}"),
                        })?;
                    self.demux_eof = true;
                }
                Err(error) => {
                    return Err(MediaError::Io {
                        message: format!("{name}: {error}"),
                    });
                }
            }
        }
    }

    /// Converts one decoded frame to the mixer's format.
    fn convert(&mut self, frame: &mut AudioFrame, name: &str) -> Result<Vec<f32>, MediaError> {
        // The converter and the frame must describe the same audio: the
        // converter is built from the frame's own parameters, and the frame
        // carries the layout and the rate the converter expects. A frame that
        // does not repeat the stream's layout or rate takes them from the
        // decoder, which knows the stream's layout from the container even
        // when the frame does not repeat it.
        let layout = canonical_layout(resolve_layout(
            frame.channel_layout(),
            self.decoder.channel_layout(),
            name,
        )?);
        let rate = if frame.rate() > 0 {
            frame.rate()
        } else {
            self.decoder.rate()
        };
        frame.set_channel_layout(layout);
        frame.set_rate(rate);
        // The L2 mixer renders mono and stereo only, so a track with more
        // channels is downmixed to stereo.
        let out_layout = if layout.channels() > 2 {
            ChannelLayout::STEREO
        } else {
            layout
        };
        let input = (frame.format(), layout.bits(), rate);
        if self.resampler_input != Some(input) {
            let converter = resampling::Context::get(
                frame.format(),
                layout,
                rate,
                Sample::F32(sample::Type::Packed),
                out_layout,
                rate,
            )
            .map_err(|error| MediaError::Decode {
                format: String::new(),
                message: format!("{name}: audio resampler: {error}"),
            })?;
            self.spec.sample_rate = rate;
            self.spec.channels = channel_count(name, out_layout.channels())?;
            self.resampler = Some(converter);
            self.resampler_input = Some(input);
        }

        match self.resampler.as_mut() {
            Some(converter) => resample_frame(converter, frame, name),
            None => Err(MediaError::Decode {
                format: String::new(),
                message: format!("{name}: the audio resampler is missing"),
            }),
        }
    }

    /// Flushes the converter's tail, the samples it still holds.
    fn flush(&mut self, name: &str) -> Result<Vec<f32>, MediaError> {
        let Some(converter) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        let Some(delay) = converter.delay() else {
            return Ok(Vec::new());
        };
        // The converter's output definition holds the layout the track is
        // converted to.
        let layout = converter.output().channel_layout;
        let tail = usize::try_from(delay.output).map_err(|_| MediaError::Decode {
            format: String::new(),
            message: format!("{name}: the audio tail length is out of range"),
        })?;
        let mut output = AudioFrame::new(Sample::F32(sample::Type::Packed), tail, layout);
        converter
            .flush(&mut output)
            .map_err(|error| MediaError::Decode {
                format: String::new(),
                message: format!("{name}: audio flush: {error}"),
            })?;
        audio_packed_f32(&output, name).map(|samples| samples.to_vec())
    }
}

/// Gives an unnamed layout its canonical name.
///
/// FFmpeg reports a PCM AU track as `channels: 1, mask: 0`; the resampler
/// compares the named layout and rejects that frame as an input change even
/// though the converter was built from the same mask. Naming mono/stereo
/// keeps the frame and the converter on the same layout.
fn canonical_layout(layout: ChannelLayout) -> ChannelLayout {
    if layout.bits() != 0 {
        return layout;
    }
    match layout.channels() {
        1 => ChannelLayout::MONO,
        2 => ChannelLayout::STEREO,
        _ => layout,
    }
}

/// The channel layout a decoded audio frame is converted from.
///
/// The frame's own layout is authoritative. A frame that carries none is
/// converted with the decoder's layout: the decoder knows the stream's
/// layout from the container even when the frame does not repeat it. A
/// track whose frame and decoder both carry no layout has an unknown
/// channel count, and the only sound layout that could be invented for it
/// would either drop half of a stereo track or read a mono track as
/// interleaved stereo, so it is reported instead.
pub(crate) fn resolve_layout(
    frame_layout: ChannelLayout,
    decoder_layout: ChannelLayout,
    name: &str,
) -> Result<ChannelLayout, MediaError> {
    if frame_layout.channels() > 0 {
        return Ok(frame_layout);
    }
    if decoder_layout.channels() > 0 {
        return Ok(decoder_layout);
    }
    Err(MediaError::InvalidData {
        message: format!("{name}: the audio track carries no channel layout"),
    })
}

/// Converts a decoder channel count to the `AudioSpec::channels` field.
///
/// The count is kept in a 32-bit field, so only a negative count cannot be
/// represented; it is reported rather than clamped.
pub(crate) fn channel_count(name: &str, channels: i32) -> Result<u32, MediaError> {
    u32::try_from(channels).map_err(|_| MediaError::InvalidData {
        message: format!("{name}: audio channel count {channels} is negative"),
    })
}

/// Runs the resampler over one decoded frame and copies the packed `f32`
/// samples out.
fn resample_frame(
    converter: &mut resampling::Context,
    frame: &AudioFrame,
    name: &str,
) -> Result<Vec<f32>, MediaError> {
    let mut output = AudioFrame::empty();
    converter
        .run(frame, &mut output)
        .map_err(|error| MediaError::Decode {
            format: String::new(),
            message: format!("{name}: audio resample: {error}"),
        })?;
    Ok(audio_packed_f32(&output, name)?.to_vec())
}

/// Borrows the single packed `f32` plane of an audio frame.
///
/// A frame with no plane carries no samples: that is a decode failure, not an
/// empty frame.
fn audio_packed_f32<'a>(frame: &'a AudioFrame, name: &str) -> Result<&'a [f32], MediaError> {
    if frame.planes() == 0 {
        return Err(MediaError::Decode {
            format: String::new(),
            message: format!("{name}: the converted audio frame carries no samples"),
        });
    }
    Ok(frame.plane::<f32>(0))
}
