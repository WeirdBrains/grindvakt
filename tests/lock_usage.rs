use grindvakt::lock::SingleInstanceLock;

#[test]
fn second_lock_fails() {
    let lock1 = SingleInstanceLock::acquire("test-lock").expect("first lock ok");
    let res = SingleInstanceLock::acquire("test-lock");
    assert!(res.is_err(), "second lock should fail");
    drop(lock1); // release
    // Now can acquire again
    let _lock2 = SingleInstanceLock::acquire("test-lock").expect("reacquire after drop");
}
