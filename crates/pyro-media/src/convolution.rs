//! Zero-latency convolution in Web Audio's 128-frame render quanta.
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

    fn add(&mut self, p: &Partition, src: &Block, dst: &mut Block) {
        let start = self.filled;
        for i in 0..QUANTUM {
            dst[i] += self.pending[start + i];
            self.input[self.index][start + i] = Complex32::new(src[i], 0.);
        }
        self.filled += QUANTUM;
        if self.filled != p.size {
            return;
        }
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

    fn add(&mut self, src: &Block, dst: &mut Block) {
        let mut samples = [0.; QUANTUM * 2];
        samples[..QUANTUM].copy_from_slice(&self.previous);
        samples[QUANTUM..].copy_from_slice(src);
        self.previous.copy_from_slice(src);
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
        const SILENCE: Block = [0.; QUANTUM];
        let stereo = right.is_some();
        let [l, r] = out;
        l.fill(0.);
        r.fill(0.);
        if self.impulse_channels == 1 {
            let stereo_output = stereo || self.stereo_tail > 0;
            if !stereo || self.mono_tail > 0 {
                self.paths[0].add(if stereo { &SILENCE } else { left }, l);
                if stereo_output && speakers {
                    r.copy_from_slice(l);
                }
            }
            if stereo_output {
                self.paths[1].add(if stereo { left } else { &SILENCE }, l);
                self.paths[2].add(right.unwrap_or(&SILENCE), r);
            }
            self.mono_tail = if stereo { self.mono_tail.saturating_sub(QUANTUM) } else { self.impulse_length - 1 };
            self.stereo_tail = if stereo { self.impulse_length - 1 } else { self.stereo_tail.saturating_sub(QUANTUM) };
            return if stereo_output { 2 } else { 1 };
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
        2
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
