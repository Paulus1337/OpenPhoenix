pub type Job<'a, T> = Box<dyn FnOnce() -> T + Send + 'a>;

pub fn run<'a, T: Send + 'a>(jobs: Vec<Job<'a, T>>) -> Vec<std::thread::Result<T>> {
    std::thread::scope(|scope| {
        let handles = jobs
            .into_iter()
            .map(|job| scope.spawn(job))
            .collect::<Vec<_>>();
        handles.into_iter().map(|handle| handle.join()).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    #[test]
    fn runtime_sized_jobs_overlap_and_results_keep_submission_order() {
        let count = 4usize;
        let barrier = Arc::new(Barrier::new(count));
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let jobs = (0..count)
            .map(|index| {
                let barrier = barrier.clone();
                let live = live.clone();
                let peak = peak.clone();
                Box::new(move || {
                    let active = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(active, Ordering::SeqCst);
                    barrier.wait();
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    live.fetch_sub(1, Ordering::SeqCst);
                    index
                }) as Job<'_, usize>
            })
            .collect();
        let results = run(jobs)
            .into_iter()
            .map(|result| result.expect("job"))
            .collect::<Vec<_>>();
        assert_eq!(results, vec![0, 1, 2, 3]);
        assert_eq!(peak.load(Ordering::SeqCst), count);
    }

    #[test]
    fn panics_are_isolated_to_their_job() {
        let results = run(vec![Box::new(|| 1), Box::new(|| panic!("boom"))]);
        assert_eq!(results[0].as_ref().ok(), Some(&1));
        assert!(results[1].is_err());
    }
}
