#![no_std]
extern crate alloc;

pub mod agent {
    use crate::intent::Action;
    use alloc::string::String;
    use alloc::vec::Vec;

    pub enum AgentState {
        Idle,
        Thinking,
        Speaking,
    }

    pub struct Interaction {
        pub prompt: String,
        pub response: String,
    }

    pub struct Companion {
        pub state: AgentState,
        pub memory: Vec<Interaction>,
    }

    pub enum Error {
        Network,
        Parse,
    }

    impl Companion {
        pub fn new() -> Self {
            Self {
                state: AgentState::Idle,
                memory: Vec::new(),
            }
        }

        // Dispatches intent to OpenAI and updates internal state
        pub async fn think(&mut self, _input: String) -> Result<Action, Error> {
            // 1. Call OpenAI via Host Function
            self.state = AgentState::Thinking;
            // 2. Parse JSON response into rclaw "Action"
            // 3. Update 'state' (e.g., Happy, Thinking, Sleepy)
            Ok(Action::Speak(String::from("Hello!")))
        }
    }
}

pub mod avatar {
    // Bridges the Agent's emotions to LVGL/3D rendering
    pub enum Expression {
        Neutral,
        Curious,
        Processing,
        Happy,
    }

    pub fn set_expression(_expr: Expression) {
        // Logic to trigger specific Lottie or 3D animations in rlvgl
    }
}

pub mod intent {
    use alloc::string::String;

    // Maps OpenAI "tools" to hardware actions
    pub enum Action {
        Move(f32, f32), // If you add servos later
        Speak(String),  // Triggers TTS
        DisplayUI(String), // Updates the 2.8" screen
    }
}
