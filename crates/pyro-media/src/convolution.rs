//! Zero-latency convolution with a fixed 128-frame head and variable render blocks.
//! The first quantum of the impulse is a direct FIR. Later partitions use
//! overlap-add: a partition of size N starts N samples into the impulse, so
//! its input block is complete before its first output is due.
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::sync::Arc;

pub const QUANTUM: usize = 128;
const MAX_PARTITION: usize = 4096;
type Block = [f32; QUANTUM];

struct Partition {
    size: usize,
    kernels: Vec<Vec<Complex32>>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
}

struct Kernel {
    head: Vec<(usize, f32)>,
    partitions: Vec<Partition>,
}

impl Kernel {
    fn new(ir: &[f32], scale: f32, planner: &mut FftPlanner<f32>) -> Self {
        let head =
            ir.iter().take(QUANTUM).enumerate().filter(|(_, x)| **x != 0.0).map(|(i, x)| (i, x * scale)).collect();
        let mut partitions = Vec::new();
        let mut offset = QUANTUM;
        while offset < ir.len() {
            let size = offset.min(MAX_PARTITION);
            // At the largest partition size continue with a frequency-domain
            // delay line. This bounds the cost of any individual FFT.
            let end = if size == MAX_PARTITION { ir.len() } else { (offset + size).min(ir.len()) };
            let forward = planner.plan_fft_forward(2 * size);
            let inverse = planner.plan_fft_inverse(2 * size);
            let mut kernels = Vec::new();
            let normalization = scale / (2 * size) as f32;
            for taps in ir[offset..end].chunks(size) {
                let mut spectrum = vec![Complex32::default(); 2 * size];
                for (bin, tap) in spectrum.iter_mut().zip(taps) {
                    bin.re = tap * normalization;
                }
                forward.process(&mut spectrum);
                kernels.push(spectrum);
            }
            partitions.push(Partition { size, kernels, forward, inverse });
            offset = end;
        }
        Self { head, partitions }
    }
}

struct PartitionState {
    index: usize,
    filled: usize,
    input: Vec<Vec<Complex32>>,
    work: Vec<Complex32>,
    scratch: Vec<Complex32>,
    pending: Vec<f32>,
    overlap: Vec<f32>,
}

impl PartitionState {
    fn new(p: &Partition) -> Self {
        let zeros = vec![Complex32::default(); 2 * p.size];
        let scratch_len = p.forward.get_inplace_scratch_len().max(p.inverse.get_inplace_scratch_len());
        Self {
            index: 0,
            filled: 0,
            input: vec![zeros.clone(); p.kernels.len()],
            work: zeros,
            scratch: vec![Complex32::default(); scratch_len],
            pending: vec![0.; p.size],
            overlap: vec![0.; p.size],
        }
    }

    fn add(&mut self, p: &Partition, src: &[f32], dst: &mut [f32]) {
        let mut offset = 0;
        while offset < src.len() {
            let count = (p.size - self.filled).min(src.len() - offset);
            for index in 0..count {
                dst[offset + index] += self.pending[self.filled + index];
                self.input[self.index][self.filled + index] = Complex32::new(src[offset + index], 0.);
            }
            offset += count;
            self.filled += count;
            if self.filled == p.size {
                self.finish_partition(p);
            }
        }
    }

    fn finish_partition(&mut self, p: &Partition) {
        self.filled = 0;
        let newest = &mut self.input[self.index];
        newest[p.size..].fill(Complex32::default());
        p.forward.process_with_scratch(newest, &mut self.scratch);
        self.work.fill(Complex32::default());
        let mut slot = self.index;
        for kernel in &p.kernels {
            for ((out, x), h) in self.work.iter_mut().zip(&self.input[slot]).zip(kernel) {
                *out += x * h;
            }
            slot = slot.checked_sub(1).unwrap_or(self.input.len() - 1);
        }
        p.inverse.process_with_scratch(&mut self.work, &mut self.scratch);
        for i in 0..p.size {
            self.pending[i] = self.work[i].re + self.overlap[i];
            self.overlap[i] = self.work[p.size + i].re;
        }
        self.index = (self.index + 1) % self.input.len();
    }
}

struct Path {
    kernel: Arc<Kernel>,
    previous: Block,
    partitions: Vec<PartitionState>,
}
impl Path {
    fn new(kernel: Arc<Kernel>) -> Self {
        let partitions = kernel.partitions.iter().map(PartitionState::new).collect();
        Self { kernel, previous: [0.; QUANTUM], partitions }
    }

    fn add(&mut self, src: &[f32], dst: &mut [f32]) {
        let mut samples = [0.; QUANTUM * 2];
        samples[..QUANTUM].copy_from_slice(&self.previous);
        samples[QUANTUM..QUANTUM + src.len()].copy_from_slice(src);
        self.previous.copy_from_slice(&samples[src.len()..src.len() + QUANTUM]);
        for &(k, value) in &self.kernel.head {
            for (output, input) in dst.iter_mut().zip(&samples[QUANTUM - k..]) {
                *output += input * value;
            }
        }
        for (state, partition) in self.partitions.iter_mut().zip(&self.kernel.partitions) {
            state.add(partition, src, dst);
        }
    }
}

pub struct Convolver {
    paths: Vec<Path>,
    impulse_channels: usize,
    impulse_length: usize,
    stereo_tail: usize,
    mono_tail: usize,
}

impl Convolver {
    /// The caller has acquired the immutable impulse channels from an AudioBuffer.
    pub fn new(impulse: &[Vec<f32>], sample_rate: f64, normalize: bool) -> Option<Self> {
        let length = impulse.first()?.len();
        let channels = impulse.len();
        if !matches!(channels, 1 | 2 | 4) || length == 0 || impulse.iter().any(|c| c.len() != length) {
            return None;
        }
        let mut scale = 1.;
        if normalize {
            let energy = impulse.iter().flatten().map(|&v| f64::from(v).powi(2)).sum::<f64>();
            let rms = (energy / (channels as f64 * length as f64)).sqrt();
            let power = if rms.is_finite() { rms.max(0.000125) } else { 0.000125 };
            scale = 0.00125 * (44100. / sample_rate) / power;
            if channels == 4 {
                scale *= 0.5;
            }
        }
        let mut planner = FftPlanner::new();
        let mut kernels: Vec<_> =
            impulse.iter().map(|ch| Arc::new(Kernel::new(ch, scale as f32, &mut planner))).collect();
        if channels == 1 {
            kernels.push(kernels[0].clone());
            kernels.push(kernels[0].clone());
        }
        Some(Self {
            paths: kernels.into_iter().map(Path::new).collect(),
            impulse_channels: channels,
            impulse_length: length,
            stereo_tail: 0,
            mono_tail: 0,
        })
    }

    /// A mono impulse preserves the input width and honors channelInterpretation
    /// when its mono response contributes to a later stereo quantum. Stereo
    /// impulses route input channels independently; four channels form a matrix.
    pub fn render(&mut self, left: &Block, right: Option<&Block>, speakers: bool, out: &mut [Block; 2]) -> usize {
        let [l, r] = out;
        self.render_block(left, right.map(|channel| channel.as_slice()), speakers, l, r)
    }

    /// Render equally sized channel slices without padding or delaying the output.
    pub fn render_block(
        &mut self,
        left: &[f32],
        right: Option<&[f32]>,
        speakers: bool,
        l: &mut [f32],
        r: &mut [f32],
    ) -> usize {
        assert_eq!(left.len(), l.len());
        assert_eq!(left.len(), r.len());
        assert!(right.is_none_or(|channel| channel.len() == left.len()));
        let stereo_output = self.impulse_channels != 1 || right.is_some() || self.stereo_tail > 0;
        for (index, ((input, l), r)) in
            left.chunks(QUANTUM).zip(l.chunks_mut(QUANTUM)).zip(r.chunks_mut(QUANTUM)).enumerate()
        {
            let start = index * QUANTUM;
            self.render_chunk(
                input,
                right.map(|channel| &channel[start..start + input.len()]),
                speakers,
                stereo_output,
                l,
                r,
            );
        }
        if stereo_output {
            2
        } else {
            1
        }
    }

    fn render_chunk(
        &mut self,
        left: &[f32],
        right: Option<&[f32]>,
        speakers: bool,
        stereo_output: bool,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        const SILENCE: Block = [0.; QUANTUM];
        let silence = &SILENCE[..left.len()];
        let stereo = right.is_some();
        l.fill(0.);
        r.fill(0.);
        if self.impulse_channels == 1 {
            if !stereo || self.mono_tail > 0 {
                self.paths[0].add(if stereo { silence } else { left }, l);
                if stereo_output && speakers {
                    r.copy_from_slice(l);
                }
            }
            if stereo_output {
                self.paths[1].add(if stereo { left } else { silence }, l);
                self.paths[2].add(right.unwrap_or(silence), r);
            }
            self.mono_tail = if stereo { self.mono_tail.saturating_sub(left.len()) } else { self.impulse_length - 1 };
            self.stereo_tail =
                if stereo { self.impulse_length - 1 } else { self.stereo_tail.saturating_sub(left.len()) };
            return;
        }

        let right = right.unwrap_or(left);
        self.paths[0].add(left, l);
        if self.impulse_channels == 4 {
            // Lout = L*IR0 + R*IR2. Rout = L*IR1 + R*IR3.
            self.paths[1].add(left, r);
            self.paths[2].add(right, l);
            self.paths[3].add(right, r);
        } else {
            self.paths[1].add(right, r);
        }
    }
}

impl Convolver {
    /// Bytes in the long-lived DSP arrays (excluding shared FFT plans).
    pub fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        let kernel_bytes: usize = self.paths[..self.impulse_channels]
            .iter()
            .map(|p| {
                p.kernel.head.capacity() * size_of::<(usize, f32)>()
                    + p.kernel
                        .partitions
                        .iter()
                        .flat_map(|partition| &partition.kernels)
                        .map(|v| v.capacity() * size_of::<Complex32>())
                        .sum::<usize>()
            })
            .sum();
        kernel_bytes
            + self
                .paths
                .iter()
                .flat_map(|p| &p.partitions)
                .map(|s| {
                    (s.work.capacity() + s.scratch.capacity() + s.input.iter().map(Vec::capacity).sum::<usize>())
                        * size_of::<Complex32>()
                        + (s.pending.capacity() + s.overlap.capacity()) * size_of::<f32>()
                })
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irregular_blocks_match_direct_convolution() {
        let mut impulse = vec![0.; 8301];
        let taps = [(0, 1.), (73, -0.5), (127, 0.25), (128, 0.5), (511, -0.25), (4096, 0.25), (8300, 0.5)];
        for (delay, gain) in taps {
            impulse[delay] = gain;
        }
        let mut signal: Vec<_> = (0..789).map(|index| (index % 23) as f32 / 23.).collect();
        signal.resize(signal.len() + impulse.len() - 1, 0.);
        let mut engine = Convolver::new(&[impulse], 48000., false).unwrap();
        let mut start = 0;
        for size in [1, 63, 129, 511, 128, 37].into_iter().cycle() {
            let end = (start + size).min(signal.len());
            let mut left = vec![0.; end - start];
            let mut right = left.clone();
            assert_eq!(engine.render_block(&signal[start..end], None, true, &mut left, &mut right), 1);
            for (frame, sample) in (start..end).zip(left) {
                let expected: f32 =
                    taps.iter().filter_map(|&(delay, gain)| frame.checked_sub(delay).map(|i| signal[i] * gain)).sum();
                assert!((sample - expected).abs() < 2e-5, "frame {frame}: {sample} != {expected}");
            }
            start = end;
            if start == signal.len() {
                break;
            }
        }
    }

    #[test]
    fn stereo_tail_counts_frames_not_quanta() {
        let mut impulse = vec![0.; 131];
        impulse[130] = 1.;
        let mut engine = Convolver::new(&[impulse], 48000., false).unwrap();
        assert_eq!(engine.render_block(&[0.], Some(&[1.]), true, &mut [0.], &mut [0.]), 2);
        assert_eq!(engine.render_block(&[0.; 129], None, true, &mut [0.; 129], &mut [0.; 129]), 2);
        let mut right = [0.];
        assert_eq!(engine.render_block(&[0.], None, true, &mut [0.], &mut right), 2);
        assert!((right[0] - 1.).abs() < 1e-6);
        assert_eq!(engine.render_block(&[0.], None, true, &mut [0.], &mut right), 1);
    }
}
