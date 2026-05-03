use super::DEPTH_STREAM_SUFFIX;
use super::model::{CombinedStream, StreamEvent};

pub fn parse_stream(text: &str) -> Option<(String, StreamEvent)> {
    let wrapper: CombinedStream = serde_json::from_str(text).ok()?;
    let event = if wrapper.stream.ends_with("@bookTicker") {
        serde_json::from_value(wrapper.data)
            .ok()
            .map(StreamEvent::BookTicker)?
    } else if wrapper.stream.contains(DEPTH_STREAM_SUFFIX) {
        serde_json::from_value(wrapper.data)
            .ok()
            .map(StreamEvent::Depth)?
    } else if wrapper.stream.ends_with("@aggTrade") {
        serde_json::from_value(wrapper.data)
            .ok()
            .map(StreamEvent::AggTrade)?
    } else {
        StreamEvent::Ignore
    };
    Some((wrapper.stream, event))
}
