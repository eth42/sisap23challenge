#![allow(dead_code)]
#![allow(unused_mut)]
#![allow(unused_variables)]

use hiob::{
	limit_threads,
	num_threads,
	pydata::H5PyDataset,
	data::MatrixDataSource,
	binarizer::StochasticHIOB,
	eval::BinarizationEvaluator,
	progress::par_iter,
};
use ndarray::{Axis, Array2, Slice};
use clap::Parser;
use std::str::FromStr;
use std::time::Instant;
use itertools::Itertools;
use num_traits::cast::NumCast;
use rayon::iter::ParallelIterator;

mod fs_fun;
// mod hdf5_fun;
mod h5py_fun;
mod cli;

use crate::fs_fun::{download_if_missing};
// use crate::hdf5_fun::store_results;
use crate::h5py_fun::store_results;
use crate::cli::{Cli};
// use crate::h5py_fun::{get_h5py_shape, get_h5py_slice_f32};

const PRODUCTION_MODE: bool = true;

type Res<T> = Result<T, Box<dyn std::error::Error>>;
type NoRes = Res<()>;

// Download all missing files for a specified format and size
fn ensure_files_available(in_base_path: &str, kind: &str, size: &str) -> NoRes {
	let base_url = if PRODUCTION_MODE {
		"http://sisap-23-challenge.s3.amazonaws.com/SISAP23-Challenge"
	} else {
		"http://ingeotec.mx/~sadit/metric-datasets/LAION/SISAP23-Challenge"
	};
	let versions = vec!["query", "dataset"];
	let urls = vec![
		format!("{}/public-queries-10k-{}.h5", base_url, kind),
		format!("{}/laion2B-en-{}-n={}.h5", base_url, kind, size),
	];
	let targets = vec![
		queries_path(in_base_path, kind),
		dataset_path(in_base_path, kind, size),
	];
	for (version,(url, target)) in versions.iter().zip(urls.iter().zip(targets.iter())) {
		download_if_missing(
			url,
			target,
		)?
	};
	Ok(())
}
fn dataset_path(in_base_path: &str, kind: &str, size: &str) -> String {
	format!("{}/{}/{}/dataset.h5", in_base_path, kind, size)
}
fn queries_path(in_base_path: &str, kind: &str) -> String {
	format!("{}/{}/query.h5", in_base_path, kind)
}
fn result_path(out_base_path: &str, kind: &str, size: &str, index_identifier: &str, param_string: &str) -> String {
	format!("{}/{}/{}/{}/{}.h5", out_base_path, kind, size, index_identifier, param_string)
}



struct Timer {
	start: Instant
}
impl Timer {
	fn new() -> Self {
		Timer{start: Instant::now()}
	}
	fn elapsed_s(&self) -> f64 {
		self.start.elapsed().as_secs_f64()
	}
	fn elapsed_str(&self) -> String {
		time_format(self.start.elapsed().as_secs_f64())
	}
}


fn linspace<T: NumCast+Copy+Clone>(start: T, end: T, n_vals: usize) -> Vec<T> {
	let fstart: f64 = <f64 as NumCast>::from(start.clone()).unwrap();
	let fend: f64 = <f64 as NumCast>::from(end.clone()).unwrap();
	let fstep = (fend-fstart)/((n_vals-1) as f64);
	let mut vals: Vec<T> = (0..n_vals)
	.map(|i_val| fstart + fstep * (i_val as f64))
	.map(|fval| <T as NumCast>::from(fval).unwrap())
	.collect();
	/* Fix limits to guarantee start and end included */
	vals[0] = start;
	vals[n_vals-1] = end;
	vals
}
fn logspace<T: NumCast+Copy+Clone>(start: T, end: T, n_vals: usize) -> Vec<T> {
	let fstart: f64 = <f64 as NumCast>::from(start.clone()).unwrap();
	let fend: f64 = <f64 as NumCast>::from(end.clone()).unwrap();
	let lstart = fstart.ln();
	let lend = fend.ln();
	let mut vals: Vec<T> = linspace(lstart, lend, n_vals)
	.iter()
	.map(|lval| lval.exp())
	.map(|fval| <T as NumCast>::from(fval).unwrap())
	.collect();
	/* Fix limits to guarantee start and end included */
	vals[0] = start;
	vals[n_vals-1] = end;
	vals
}


// fn read_h5py_source(file: &str, dataset: &str, batch_size: usize) -> Res<Array2<f32>> {
// 	let data_shape = get_h5py_shape(file, dataset)?;
// 	let mut data = Array2::from_elem((data_shape[0], data_shape[1]), 0f32);
// 	let n_batches = (data_shape[0]+(batch_size-1)) / batch_size;
// 	par_iter(
// 		(0..n_batches)
// 		.map(|i_batch| {
// 			let batch_start = batch_size * i_batch;
// 			let batch_end = (batch_start + batch_size).min(data_shape[0]);
// 			(batch_start, batch_end)
// 		})
// 		.zip(data.axis_chunks_iter_mut(Axis(0), batch_size))
// 	)
// 	.for_each(|((batch_start, batch_end), mut data_chunk)| {
// 		let batch = get_h5py_slice_f32(file, dataset, batch_start, batch_end).unwrap();
// 		batch.axis_iter(Axis(0))
// 		.zip(data_chunk.axis_iter_mut(Axis(0)))
// 		.for_each(|(from, mut to)| {
// 			to.assign(&from);
// 		});
// 	});
// 	Ok(data)
// }

fn read_h5py_source(source: &H5PyDataset<f32>, batch_size: usize) -> Array2<f32> {
	let data_shape = [source.n_rows(), source.n_cols()];
	let mut data = Array2::from_elem(data_shape, 0f32);
	let n_batches = (data_shape[0]+(batch_size-1)) / batch_size;
	par_iter(
		(0..n_batches)
		.map(|i_batch| {
			let batch_start = batch_size * i_batch;
			let batch_end = (batch_start + batch_size).min(data_shape[0]);
			(batch_start, batch_end)
		})
		.zip(data.axis_chunks_iter_mut(Axis(0), batch_size))
	)
	.for_each(|((batch_start, batch_end), mut data_chunk)| {
		let batch = source.get_rows_slice(batch_start, batch_end);
		batch.axis_iter(Axis(0))
		.zip(data_chunk.axis_iter_mut(Axis(0)))
		.for_each(|(from, mut to)| {
			to.assign(&from);
		});
	});
	data
}
fn time_format(seconds: f64) -> String {
	let ms = ((seconds%1f64)*1000f64).floor();
	let s = (seconds%60f64).floor();
	let m = ((seconds/60f64)%60f64).floor();
	let h = (seconds/3600f64).floor();
	match (s < 1f64, m < 1f64, h < 1f64) {
		(true, _, _) => format!("{:.0}ms", ms),
		(false, true, _) => format!("{:.0}s{:03.0}ms", s, ms),
		(false, false, true) => format!("{:.0}m{:02.0}s{:03.0}ms", m, s, ms),
		(false, false, false) => format!("{:.0}h{:02.0}m{:02.0}s{:03.0}ms", h, m, s, ms),
	}
}


fn run_experiment(
	in_base_path: &str,
	out_base_path: &str,
	kind: &str,
	key: &str,
	size: &str,
	k: usize,
	ram_mode: bool,
	n_bitss: &Vec<usize>,
	n_its: usize,
	sample_size: usize,
	its_per_sample: usize,
	noise_std: f32,
	nprobe_vals: &Vec<usize>,
) -> NoRes {
	println!("Running {}", kind);
	ensure_files_available(in_base_path, kind, size)?;

	assert!(ram_mode);

	let data_path = dataset_path(in_base_path, kind, size);
	let queries_path = queries_path(in_base_path, kind);
	let data_file: H5PyDataset<f32> = H5PyDataset::new(data_path.as_str(), key);
	let queries_file: H5PyDataset<f32> = H5PyDataset::new(queries_path.as_str(), key);
	// let data_shape = get_h5py_shape(data_path.as_str(), key)?;
	// let queries_shape = get_h5py_shape(queries_path.as_str(), key)?;
	let data_shape = [data_file.n_rows(), data_file.n_cols()];
	let queries_shape = [queries_file.n_rows(), queries_file.n_cols()];


	/* Training */
	println!("Training index on {:?} with {:?} bits",data_shape,n_bitss);
	let build_timer = Timer::new();
	/* Creating string handle for experiment */
	let index_identifier = format!(
		"StochasticHIOB(n_bits={:?},n_its={:},n_samples={:},batch_its={:},noise_std={:})",
		n_bitss,
		n_its,
		sample_size,
		its_per_sample,
		noise_std
	);
	/* Loading data */
	let data_load_timer = Timer::new();
	// let data = read_h5py_source(data_path.as_str(), key, 300_000)?;
	let data = read_h5py_source(&data_file, 300_000);
	println!("Data loaded in {:}", data_load_timer.elapsed_str());
	/* Training HIOBs */
	let mut hs = vec![];
	let mut data_bins = vec![];
	(0..n_bitss.len()).for_each(|i_hiob| {
		let init_timer = Timer::new();
		let mut h: StochasticHIOB<f32, u64, &Array2<f32>> = StochasticHIOB::new(
			&data,
			sample_size,
			its_per_sample,
			*n_bitss.get(i_hiob).unwrap(),
			None,
			Some(0.1),
			None,
			None,
			Some(true),
			None,
			None,
			if noise_std > 0f32 { Some(noise_std) } else { None },
		);
		println!("Stochastic HIOB {} initialized in {:}", i_hiob+1, init_timer.elapsed_str());
		let init_timer = Timer::new();
		h.run(n_its);
		println!("Stochastic HIOB {} trained in {:}", i_hiob+1, init_timer.elapsed_str());
		let init_timer = Timer::new();
		let data_bin = h.binarize(&data);
		println!("Data binarized with HIOB {} in {:}", i_hiob+1, init_timer.elapsed_str());
		hs.push(h);
		data_bins.push(data_bin);
	});
	let build_time = build_timer.elapsed_s();
	println!("Done training in {:}.", time_format(build_time.clone()));
	
	let nprobe_groups: Vec<Vec<usize>> = nprobe_vals.clone().into_iter().rev().combinations(n_bitss.len()).collect();
	let bin_eval = BinarizationEvaluator::new();
	for nprobes in nprobe_groups.iter() {
		println!("Starting search on {:?} with nprobes={:?}", queries_shape, nprobes);
		let query_timer = Timer::new();
		/* Loading queries */
		let queries_load_timer = Timer::new();
		let queries = read_h5py_source(&queries_file, 300_000);
		// let queries = read_h5py_source(queries_path.as_str(), key, 300_000)?;
		println!("Queries loaded in {:}", queries_load_timer.elapsed_str());
		/* Binarize queries */
		let queries_bin_timer = Timer::new();
		let queries_bins: Vec<Array2<u64>> = hs.iter()
		.map(|h| h.binarize(&queries))
		.collect();
		println!("Queries binarized in {:}", queries_bin_timer.elapsed_str());
		/* Perform query */
		let query_call_timer = Timer::new();
		let chunk_size = (queries_shape[0]+num_threads()*2-1)/(num_threads()*2);
		let (mut neighbor_dists, mut neighbor_ids) = bin_eval.query_cascade(
			&data,
			&data_bins,
			&queries,
			&queries_bins,
			k,
			nprobes,
			Some(chunk_size),
		);
		println!("Queries executed in {:}", query_call_timer.elapsed_str());
		/* Modify dot products to euclidean distances and change to 1-based index */
		neighbor_dists.mapv_inplace(|v| 0f32.max(2f32-2f32*v).sqrt());
		neighbor_ids.mapv_inplace(|v| v+1);
		let query_time = query_timer.elapsed_s();
		println!("Overall query time: {:}", time_format(query_time.clone()));
		/* Create parameter string and store results */
		let param_string = format!(
			"index_params=({:}),query_params=(nprobe={:?})",
			format!(
				"scale%={:},its_per_sample={:}",
				<usize as NumCast>::from(hs.get(0).unwrap().get_scale()*100f32).unwrap(),
				its_per_sample,
			),
			nprobes,
		);
		let out_file = result_path(out_base_path, kind, size, index_identifier.as_str(), param_string.as_str());
		let storage_timer = Timer::new();
		store_results(
			out_file.as_str(),
			kind,
			size,
			format!("{} + brute-force", index_identifier).as_str(),
			param_string.as_str(),
			neighbor_dists,
			neighbor_ids,
			&data_bins[data_bins.len()-1],
			&queries_bins[data_bins.len()-1],
			build_time,
			query_time,
		)?;
		println!("Wrote results to disk in {:}", storage_timer.elapsed_str());
	}
	Ok(())
}


fn run_experiment_single(
	in_base_path: &str,
	out_base_path: &str,
	kind: &str,
	key: &str,
	size: &str,
	k: usize,
	ram_mode: bool,
	n_bits: usize,
	n_its: usize,
	sample_size: usize,
	its_per_sample: usize,
	noise_std: f32,
	nprobe_vals: &Vec<usize>,
) -> NoRes {
	println!("Running {}", kind);
	ensure_files_available(in_base_path, kind, size)?;

	assert!(ram_mode);

	let data_path = dataset_path(in_base_path, kind, size);
	let queries_path = queries_path(in_base_path, kind);
	let data_file: H5PyDataset<f32> = H5PyDataset::new(data_path.as_str(), key);
	let queries_file: H5PyDataset<f32> = H5PyDataset::new(queries_path.as_str(), key);
	// let data_shape = get_h5py_shape(data_path.as_str(), key)?;
	// let queries_shape = get_h5py_shape(queries_path.as_str(), key)?;
	let data_shape = [data_file.n_rows(), data_file.n_cols()];
	let queries_shape = [queries_file.n_rows(), queries_file.n_cols()];


	/* Training */
	println!("Training index on {:?} with {:?} bits",data_shape,n_bits);
	let build_timer = Timer::new();
	/* Creating string handle for experiment */
	let index_identifier = format!(
		"StochasticHIOB(n_bits=[{:?}],n_its={:},n_samples={:},batch_its={:},noise_std={:})",
		n_bits,
		n_its,
		sample_size,
		its_per_sample,
		noise_std
	);
	/* Loading data */
	let data_load_timer = Timer::new();
	// let data = read_h5py_source(data_path.as_str(), key, 300_000)?;
	let data = read_h5py_source(&data_file, 300_000);
	println!("Data loaded in {:}", data_load_timer.elapsed_str());
	/* Training HIOB */
	let init_timer = Timer::new();
	let mut h: StochasticHIOB<f32, u64, &Array2<f32>> = StochasticHIOB::new(
		&data,
		sample_size,
		its_per_sample,
		n_bits,
		None,
		Some(0.1),
		None,
		None,
		Some(true),
		None,
		None,
		if noise_std > 0f32 { Some(noise_std) } else { None },
	);
	println!("Stochastic HIOB initialized in {:}", init_timer.elapsed_str());
	let init_timer = Timer::new();
	h.run(n_its);
	println!("Stochastic HIOB trained in {:}", init_timer.elapsed_str());
	let init_timer = Timer::new();
	let data_bin = h.binarize(&data);
	println!("Data binarized in {:}", init_timer.elapsed_str());
	let build_time = build_timer.elapsed_s();
	println!("Done training in {:}.", time_format(build_time.clone()));
	
	let max_nprobes = nprobe_vals.iter().max().unwrap();
	let bin_eval = BinarizationEvaluator::new();
	println!("Starting search on {:?}", queries_shape);
	let query_timer = Timer::new();
	/* Loading queries */
	let queries_load_timer = Timer::new();
	let queries = read_h5py_source(&queries_file, 300_000);
	// let queries = read_h5py_source(queries_path.as_str(), key, 300_000)?;
	println!("Queries loaded in {:}", queries_load_timer.elapsed_str());
	/* Binarize queries */
	let queries_bin_timer = Timer::new();
	let queries_bin: Array2<u64> = h.binarize(&queries);
	println!("Queries binarized in {:}", queries_bin_timer.elapsed_str());
	/* Precompute candidates for all nprobes */
	let query_call_timer = Timer::new();
	let chunk_size = (queries_shape[0]+num_threads()*2-1)/(num_threads()*2);
	let (_, all_candidates) = bin_eval.brute_force_k_smallest_hamming(&data_bin, &queries_bin, *max_nprobes, Some(chunk_size));
	println!("Candidates precomputed in {:}", query_call_timer.elapsed_str());
	for nprobes in nprobe_vals.iter() {
		println!("Refining with nprobes={:?}", nprobes);
		/* Perform query */
		let query_call_timer = Timer::new();
		let (mut neighbor_dists, mut neighbor_ids) = bin_eval.refine(
			&data,
			&queries,
			&all_candidates.slice_axis(Axis(1),Slice::from(0..*nprobes)),
			k,
			Some(chunk_size),
		);
		let query_call_time = query_call_timer.elapsed_s();
		println!("Queries executed in {:}", time_format(query_call_time.clone()));
		/* Modify dot products to euclidean distances and change to 1-based index */
		neighbor_dists.mapv_inplace(|v| 0f32.max(2f32-2f32*v).sqrt());
		neighbor_ids.mapv_inplace(|v| v+1);
		/* Create parameter string and store results */
		let param_string = format!(
			"index_params=({:}),query_params=(nprobe=[{:?}])",
			format!(
				"scale%={:},its_per_sample={:}",
				<usize as NumCast>::from(h.get_scale()*100f32).unwrap(),
				its_per_sample,
			),
			nprobes,
		);
		let out_file = result_path(out_base_path, kind, size, index_identifier.as_str(), param_string.as_str());
		let storage_timer = Timer::new();
		store_results(
			out_file.as_str(),
			kind,
			size,
			format!("{} + brute-force", index_identifier).as_str(),
			param_string.as_str(),
			neighbor_dists,
			neighbor_ids,
			&data_bin,
			&queries_bin,
			build_time,
			query_call_time,
		)?;
		println!("Wrote results to disk in {:}", storage_timer.elapsed_str());
	}
	println!("Overall query time: {:}", query_timer.elapsed_str());
	Ok(())
}

fn main() -> NoRes {
	let args = Cli::parse();
	assert!(args.idle_cpus < num_cpus::get());
	let _ = limit_threads(num_cpus::get()-args.idle_cpus);
	let probe_vals = logspace(args.probe_min, args.probe_max, args.probe_steps);
	if args.tune {
		println!("Running hyperparameter tuning mode with probes {:?}", probe_vals);
		run_experiment_single(
			args.in_path.as_str(),
			args.out_path.as_str(),
			"clip768v2",
			"emb",
			args.size.as_str(),
			args.k,
			args.ram,
			args.bits.split(",").map(|v| usize::from_str(v.trim()).unwrap()).collect::<Vec<usize>>()[0],
			args.its,
			args.samples,
			args.batch_its,
			args.noise,
			&probe_vals,
		)?;
	} else {
		println!("Running \"production\" mode with probes {:?}", probe_vals);
		run_experiment(
			args.in_path.as_str(),
			args.out_path.as_str(),
			"clip768v2",
			"emb",
			args.size.as_str(),
			args.k,
			args.ram,
			&args.bits.split(",").map(|v| usize::from_str(v.trim()).unwrap()).collect::<Vec<usize>>(),
			args.its,
			args.samples,
			args.batch_its,
			args.noise,
			&probe_vals,
		)?;
	}
	Ok(())
}
