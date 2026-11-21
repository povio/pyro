use pyro_media::convolution::{Convolver, QUANTUM};

#[test]
fn partitioned_output_matches_linear_convolution_through_the_tail() {
    // Two samples around each partition boundary, including multiple tail
    // partitions and a partial final partition.
    let mut impulse = vec![0.; 9723];
    for (i, v) in impulse.iter_mut().enumerate() {
        *v = ((i * 13 % 53) as f32 - 26.) / 1024.;
    }
    let signal: Vec<_> = (0..513).map(|i| ((i * 7 % 47) as f32 - 23.) / 32.).collect();
    let mut expected = vec![0f64; impulse.len() + signal.len() - 1];
    for (i, &x) in signal.iter().enumerate() {
        for (j, &h) in impulse.iter().enumerate() {
            expected[i + j] += f64::from(x) * f64::from(h);
        }
    }
    let mut filter = Convolver::new(&[impulse], 48000., false).unwrap();
    let mut channels = [[0.; QUANTUM]; 2];
    for start in (0..expected.len() + QUANTUM).step_by(QUANTUM) {
        let mut block = [0.; QUANTUM];
        for (i, value) in block.iter_mut().enumerate() {
            *value = signal.get(start + i).copied().unwrap_or(0.);
        }
        assert_eq!(filter.render(&block, None, true, &mut channels), 1);
        for (i, &value) in channels[0].iter().enumerate() {
            let want = expected.get(start + i).copied().unwrap_or(0.);
            assert!((f64::from(value) - want).abs() < 2e-5, "at {}: {value} != {want}", start + i);
        }
    }
}

#[test]
fn stereo_matrix_and_normalization() {
    let ir = [vec![1.], vec![2.], vec![3.], vec![4.]];
    let scale = 0.00125 / (7.5f32).sqrt() * 0.5;
    for normalize in [false, true] {
        let mut filter = Convolver::new(&ir, 44100., normalize).unwrap();
        let mut channels = [[0.; QUANTUM]; 2];
        let gain = if normalize { scale } else { 1. };
        assert_eq!(filter.render(&[1.; QUANTUM], Some(&[2.; QUANTUM]), true, &mut channels), 2);
        assert!((channels[0][0] - 7. * gain).abs() < 1e-6);
        assert!((channels[1][0] - 10. * gain).abs() < 1e-6);
    }
}

#[test]
fn mono_tail_obeys_the_current_layout_when_a_stereo_input_arrives() {
    for speakers in [false, true] {
        let mut ir = vec![0.; QUANTUM + 1];
        ir[QUANTUM] = 1.;
        let mut engine = Convolver::new(&[ir], 48000., false).unwrap();
        let mut sample = [0.; QUANTUM];
        sample[0] = 0.75;
        let mut out = [[0.; QUANTUM]; 2];
        assert_eq!(engine.render(&sample, None, !speakers, &mut out), 1);
        assert_eq!(engine.render(&[0.; QUANTUM], Some(&[0.; QUANTUM]), speakers, &mut out), 2);
        assert!((out[0][0] - 0.75).abs() < 1e-6);
        assert!((out[1][0] - if speakers { 0.75 } else { 0. }).abs() < 1e-6);
    }
}
