use super::*;

#[test]
fn test_executor_config_default() {
    let config = ExecutorConfig::default();
    assert!(matches!(config.mode, ExecutorMode::SingleThreaded));
    assert!(config.fair_scheduling);
}

#[test]
fn test_task_markers_default() {
    let markers = TaskMarkers::default();
    assert!(markers.is_sendable);
    assert!(markers.is_shareable);
}

#[test]
fn test_spawn_and_start_task() {
    let executor = ThreadedExecutor::new(ExecutorConfig::default());
    executor.register_callback(|_handle| 42);

    let handle = executor.spawn(100, TaskMarkers::default());
    assert!(executor.push_arg(handle, 1));
    assert!(executor.push_arg(handle, 2));
    assert!(executor.start(handle));

    let completed = executor.run_until_quiescent();
    assert_eq!(completed, 1);

    match executor.get_state(handle) {
        Some(MTTaskState::Completed(result)) => assert_eq!(result, 42),
        other => panic!("Expected Completed(42), got {:?}", other),
    }
}

#[test]
fn test_task_cancellation() {
    let executor = ThreadedExecutor::new(ExecutorConfig::default());

    let handle = executor.spawn(100, TaskMarkers::default());
    assert!(executor.cancel_task(handle));

    match executor.get_state(handle) {
        Some(MTTaskState::Cancelled) => {}
        other => panic!("Expected Cancelled, got {:?}", other),
    }
}

#[test]
fn test_task_blocking_and_wake() {
    let executor = ThreadedExecutor::new(ExecutorConfig::default());
    executor.register_callback(|_| 0);

    let task1 = executor.spawn(1, TaskMarkers::default());
    let task2 = executor.spawn(2, TaskMarkers::default());

    executor.start(task1);
    executor.block_task(task1, MTBlockReason::AwaitingTask(task2));

    match executor.get_state(task1) {
        Some(MTTaskState::Blocked(_)) => {}
        other => panic!("Expected Blocked, got {:?}", other),
    }

    // Complete task2 should wake task1
    executor.complete_task(task2, 0);

    match executor.get_state(task1) {
        Some(MTTaskState::Pending) => {}
        other => panic!("Expected Pending after wake, got {:?}", other),
    }
}

#[test]
fn test_thread_affine_task() {
    let executor = ThreadedExecutor::new(ExecutorConfig::default());
    executor.register_callback(|_| 0);

    let markers = TaskMarkers {
        is_sendable: false, // Thread-affine
        is_shareable: true,
        origin_thread: 1,
    };

    let handle = executor.spawn(100, markers);
    executor.start(handle);

    // Should complete in single-threaded mode
    let completed = executor.run_until_quiescent();
    assert_eq!(completed, 1);
}

#[test]
fn test_channel_waiters() {
    let executor = ThreadedExecutor::new(ExecutorConfig::default());
    executor.register_callback(|_| 0);

    let task = executor.spawn(1, TaskMarkers::default());
    let channel = 1000i64;

    executor.start(task);
    executor.block_task(task, MTBlockReason::AwaitingChannelRecv(channel));

    // Wake via channel send
    assert!(executor.wake_channel_recv_waiter(channel));

    match executor.get_state(task) {
        Some(MTTaskState::Pending) => {}
        other => panic!("Expected Pending, got {:?}", other),
    }
}

#[test]
fn test_executor_stats() {
    let executor = ThreadedExecutor::new(ExecutorConfig::default());
    executor.register_callback(|_| 0);

    executor.spawn(1, TaskMarkers::default());
    executor.spawn(2, TaskMarkers::default());

    let stats = executor.stats();
    assert_eq!(stats.pending, 2);
    assert_eq!(stats.running, 0);
    assert_eq!(stats.blocked, 0);
}

#[test]
fn test_executor_reset() {
    let executor = ThreadedExecutor::new(ExecutorConfig::default());

    executor.spawn(1, TaskMarkers::default());
    executor.spawn(2, TaskMarkers::default());

    executor.reset();

    let stats = executor.stats();
    assert_eq!(stats.pending, 0);
}

#[test]
fn test_multi_threaded_config() {
    let config = ExecutorConfig {
        mode: ExecutorMode::MultiThreaded { num_threads: 4 },
        ..Default::default()
    };

    let executor = ThreadedExecutor::new(config);
    assert_eq!(executor.num_workers, 4);
}
