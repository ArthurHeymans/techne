//! Frame limits apply before anything is written, in both directions.

use techne_node::protocol::{MAX_FRAME, Message, Reply, Response, read_frame, write_frame, write_response};

#[test]
fn outgoing_frames_stop_at_the_limit() {
    techne_process::runtime().block_on(async {
        // A MessagePack string of this size has a five-byte header.
        let mut wire = Vec::new();
        write_frame(&mut wire, &"x".repeat(MAX_FRAME - 5)).await.unwrap();
        assert_eq!(wire.len(), MAX_FRAME + 4);
        let decoded = read_frame::<String>(&mut wire.as_slice()).await.unwrap().unwrap();
        assert_eq!(decoded.len(), MAX_FRAME - 5);
        wire.clear();
        let err = write_frame(&mut wire, &"x".repeat(MAX_FRAME)).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("frame exceeds"), "{err}");
        assert!(wire.is_empty(), "refusal must leave the stream untouched");
    });
}

#[test]
fn oversized_responses_leave_a_small_error_and_a_usable_stream() {
    techne_process::runtime().block_on(async {
        let mut wire = Vec::new();
        write_response(&mut wire, &Message { id: 7, body: Ok(Reply::Text("x".repeat(MAX_FRAME))) }).await.unwrap();
        assert!(wire.len() < 256);
        write_response(&mut wire, &Message { id: 8, body: Ok(Reply::Unit) }).await.unwrap();
        let mut wire = wire.as_slice();
        let refused = read_frame::<Message<Response>>(&mut wire).await.unwrap().unwrap();
        assert_eq!(refused.id, 7);
        assert!(refused.body.unwrap_err().contains("frame exceeds"));
        let next = read_frame::<Message<Response>>(&mut wire).await.unwrap().unwrap();
        assert_eq!(next.id, 8);
        assert!(matches!(next.body, Ok(Reply::Unit)));
    });
}
