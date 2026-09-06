use runtime_domain::agent::{
    AgentId, AgentObservationId, AgentObservationRequestId, AgentPermissionTarget,
    AgentProjectionEvent, AgentRuntimeGeneration, AgentTurnId,
};
use runtime_domain::session::{RuntimeCommand, RuntimeEvent, RuntimeTarget};

use super::support::*;

#[test]
fn agent_projection_commands_route_through_runtime_event_port() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions::default());

    RuntimeCommandPort::dispatch_runtime_command(
        &mut coordinator,
        RuntimeCommand::ObserveAgents {
            request_id: AgentObservationRequestId::new(7),
        },
    )
    .expect("observe agents should be accepted");

    let events = RuntimeEventPort::drain_runtime_events(&mut coordinator);
    let snapshot = events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::AgentProjection(projection) => match &**projection {
                AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
                    request_id,
                    snapshot,
                } => Some((*request_id, snapshot.clone())),
                _ => None,
            },
            _ => None,
        })
        .expect("overview snapshot should reach the runtime event port");
    assert_eq!(snapshot.0, AgentObservationRequestId::new(7));
    assert!(snapshot.1.rows.is_empty());
    assert_eq!(snapshot.1.generation, AgentRuntimeGeneration::new(1));

    // 未知 Agent 的 per-agent observation fail closed，request id 原样回显。
    RuntimeCommandPort::dispatch_runtime_command(
        &mut coordinator,
        RuntimeCommand::ObserveAgentTranscript {
            request_id: AgentObservationRequestId::new(8),
            agent_id: AgentId::new(99),
        },
    )
    .expect("observe transcript should be accepted");
    let events = RuntimeEventPort::drain_runtime_events(&mut coordinator);
    assert!(
        events.iter().any(|event| matches!(
            event,
            RuntimeEvent::AgentProjection(projection)
                if matches!(
                    &**projection,
                    AgentProjectionEvent::AgentObservationRejected {
                        request_id,
                        ..
                    } if *request_id == AgentObservationRequestId::new(8)
                )
        )),
        "unknown agent observation should be rejected without raw errors"
    );

    // StopObserving* 幂等：unknown/mismatched id 静默接受，不产生事件。
    RuntimeCommandPort::dispatch_runtime_command(
        &mut coordinator,
        RuntimeCommand::StopObservingAgents {
            observation_id: AgentObservationId::new(999),
            generation: AgentRuntimeGeneration::new(1),
        },
    )
    .expect("stop observing should be accepted");
    RuntimeCommandPort::dispatch_runtime_command(
        &mut coordinator,
        RuntimeCommand::StopObservingAgentTranscript {
            observation_id: AgentObservationId::new(999),
            generation: AgentRuntimeGeneration::new(1),
        },
    )
    .expect("stop observing transcript should be accepted");
    assert!(RuntimeEventPort::drain_runtime_events(&mut coordinator).is_empty());

    // 未知 child 的 typed permission response 返回 closed receipt。
    let rejection = RuntimeCommandPort::dispatch_runtime_command(
        &mut coordinator,
        RuntimeCommand::RespondAgentPermission {
            target: AgentPermissionTarget {
                agent_id: AgentId::new(99),
                turn_id: AgentTurnId::new(1),
                generation: AgentRuntimeGeneration::new(1),
                runtime_target: RuntimeTarget::provider("local", "qwen3"),
                request_id: "perm-1".to_string(),
            },
            option_id: Some("allow-1".to_string()),
        },
    )
    .expect_err("unknown agent permission must fail closed");
    assert_eq!(rejection, "Unknown child Agent");

    // 未知 child 的 typed stop 返回 closed receipt；main identity 不走 child stop。
    for agent_id in [AgentId::new(99), AgentId::MAIN] {
        let rejection = RuntimeCommandPort::dispatch_runtime_command(
            &mut coordinator,
            RuntimeCommand::StopAgent {
                agent_id,
                generation: AgentRuntimeGeneration::new(1),
            },
        )
        .expect_err("typed stop must fail closed for unknown/main identities");
        assert_eq!(rejection, "Unknown child Agent");
    }

    // main permission 路由不受新命令影响：原 RespondPermission 仍走 main 语义
    //（无活跃 turn 时返回 main 路径错误，而不是 child closed receipt）。
    let main_rejection = RuntimeCommandPort::dispatch_runtime_command(
        &mut coordinator,
        RuntimeCommand::respond_permission(
            RuntimeTarget::provider("local", "qwen3"),
            "main-perm-1",
            None,
        ),
    )
    .expect_err("main permission routing should stay unchanged");
    assert_eq!(main_rejection, "Conversation is not running: qwen3");
}
