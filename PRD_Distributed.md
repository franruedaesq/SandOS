# Product Requirements Document (PRD): Distributed SandOS Architecture

## 1. Executive Summary

The objective is to evolve SandOS into a distributed, cluster-based robotics operating system utilizing a unified message-driven architecture. By transitioning SandOS from direct hardware control to a localized network router, the OS becomes a cohesive entity capable of scaling from a single, standalone node to a multi-node cluster. This architecture, powered by the **SandBus**, decouples heavy logic, UI, and real-time hardware control across specialized physical nodes, improving modularity, reducing wiring, and preventing CPU bottlenecks.

## 2. Core Architecture: The Unified Message-Driven Architecture

*   **The OS as a Message Broker:** SandOS moves away from direct hardware control and becomes a localized network router via the **SandBus**, a unified Publisher/Subscriber (PubSub) messaging layer.
*   **The SandBus:** Instead of the Wasm application executing a hardware command, it publishes an intent (e.g., *"Topic: System Status. Payload: I am happy."*). The SandBus acts as a traffic cop, using a routing table to determine where the message goes.
*   **Unified ABI:** The Wasm Host-Guest interface must be standardized around this event-driven model (`host_publish`, `host_subscribe`) rather than specific hardware calls.

## 3. Phased Implementation Plan

### Phase 1: The "Loopback" Architecture (Single Node)

**Objective:** Prove the SandBus architecture natively on a single microcontroller before introducing a second board.

*   **Internal Routing:** When the Wasm sandbox publishes a message, the SandBus checks its registry. If the topic (e.g., "Expression") is handled by a local hardware peripheral, it routes the message internally.
*   **Memory Channels:** Instead of broadcasting over the radio, the SandBus routes the message through internal memory using high-speed, asynchronous channels (e.g., Rust's `mpsc`). Core 1 picks it up and drives the hardware.
*   **Hardware Agnosticism:** The Wasm application becomes 100% hardware-agnostic, publishing state without needing to know if the target is a locally wired I2C screen or a remote monitor.
*   **Phase 1 Success Criteria:** A Wasm application running on a single ESP32-S3 publishes an intent via the unified ABI. The SandBus routes it internally via memory channels to Core 1, which successfully executes the hardware action (e.g., updating an OLED display).

### Phase 2: The Distributed Architecture (Multi-Node Cluster)

**Objective:** Scale the system by adding peripheral nodes, using the identical core Wasm application and simply updating the host's routing table.

*   **Radio Extension:** When the Wasm sandbox publishes an intent, the SandBus identifies that the target module is no longer local.
*   **Seamless Translation:** The OS automatically serializes the payload, wraps it in a low-latency radio packet, and broadcasts it over the connectionless radio protocol (ESP-NOW).
*   **Role-Based Nodes:**
    *   **The Brain Node:** Runs the Wasm sandbox, processes heavy logic/AI, dictates state, and publishes to the SandBus.
    *   **The Peripheral Nodes:** "Dumb" terminals that run no Wasm. They subscribe to the radio bus, listen for specific topics (e.g., screen rendering, motor control), and execute them in hard real-time.
*   **Phase 2 Success Criteria:** The Brain Node broadcasts an intent. A Peripheral Node successfully receives it, deserializes the payload, and executes the associated hard real-time hardware action without any changes to the core Wasm logic.

## 4. Why We Are Going There (The Rationale)

*   **Ultimate Future-Proofing:** A single-node system built today can be trivially split into multiple physical devices (e.g., separating screen and buttons) later by simply flipping a configuration flag in the router, without rewriting the core OS.
*   **Modularity & Swappability:** A robot's physical modules (e.g., "head" or "arm") can be completely replaced. As long as the new module subscribes to the correct SandBus topics, the central brain is unaffected.
*   **Isolation of Bottlenecks:** Heavy UI rendering or polling complex sensors occurs on entirely different physical chips, ensuring they never steal CPU cycles from the main brain's logic.

## 5. Critical Engineering Constraints

To make this distributed OS viable and flawless across both internal memory and external radio waves, the following constraints must be strictly adhered to:

*   **Ultra-Compact Serialization:** Because the radio protocol is limited to tiny payloads (e.g., 250 bytes), strict, binary serialization is required. Complex intents must be packed into microscopic packets for travel across memory or through the air. Bloated formats like JSON are strictly prohibited.
*   **Graceful Degradation:** If a peripheral node drops off the wireless network, the main brain must not panic. The system must handle unacknowledged packets gracefully and sync back up instantly when the node returns.
*   **Unified ABI:** The Wasm Host-Guest interface must be completely standardized around this event-driven model (`host_publish`, `host_subscribe`) rather than specific hardware calls.

---
By the end of Phase 2, SandOS will have successfully evolved from a monolithic system into a unified message-driven architecture, functional natively on a single node via "loopback" routing, and instantly scalable into a distributed, role-based cluster. All firmware builds must compile successfully and be flashable using `cargo run --release`.
