use jiff::SignedDuration;
use tanuki::domain::{
    BatchError, ExpiryUpdate, NonNegativeDuration, TopicPath, Value, WriteBatch, WriteOperation,
};

#[test]
fn timer_durations_are_nonnegative_by_construction() {
    assert!(NonNegativeDuration::new(SignedDuration::ZERO).is_ok());
    assert!(NonNegativeDuration::new(SignedDuration::from_secs(30)).is_ok());
    assert!(NonNegativeDuration::new(SignedDuration::from_secs(-1)).is_err());
}

#[test]
fn write_batches_are_nonempty_by_construction() {
    assert_eq!(WriteBatch::new(Vec::new()), Err(BatchError::Empty));

    let operation = WriteOperation::PublishState {
        topic: TopicPath::parse("/battery/laptop").unwrap(),
        value: Value::Integer(82),
        expiry: ExpiryUpdate::Clear,
    };
    let batch = WriteBatch::new(vec![operation.clone()]).unwrap();
    assert_eq!(batch.operations(), &[operation]);
}
