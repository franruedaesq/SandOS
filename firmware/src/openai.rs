pub async fn route_audio_to_openai() {
    log::info!("Stub: Routing audio to OpenAI via embassy-net HTTP POST");
}

pub async fn stream_tts() {
    log::info!("Stub: Streaming TTS from OpenAI");
}

pub fn dispatch_intent(intent: &str) -> i32 {
    log::info!("Dispatching intent: {}", intent);
    0
}
