//! contract test 使用的最小 child process fixture；不属于 production composition。

use std::{
    io::{self, Write},
    time::Duration,
};

use extension_protocol::{
    BeforeTurnHookParams, BeforeTurnHookResult, ExtensionCapability, ExtensionMethod,
    ExtensionRequest, ExtensionResponse, FrameCodec, HookCancelResult, HookDescriptor, HookPhase,
    HooksListResult, InitializeResult, ToolContent, ToolDescriptor, ToolExecuteResult,
    ToolsListResult,
};
use serde_json::json;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let codec = FrameCodec::default();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin = stdin.lock();
    let mut stdout = stdout.lock();

    if mode == "exit" {
        return;
    }
    if mode == "malformed" {
        let _ = stdout.write_all(b"not-a-frame\n\n");
        let _ = stdout.flush();
        std::thread::sleep(Duration::from_secs(2));
        return;
    }
    if mode == "oversized" {
        let _ = stdout.write_all(b"Content-Length: 1024\r\n\r\n");
        let _ = stdout.flush();
        std::thread::sleep(Duration::from_secs(2));
        return;
    }
    if mode == "stderr" {
        eprintln!("fixture stderr must never cross the transport boundary");
        return;
    }
    if mode == "hold" {
        std::thread::sleep(Duration::from_secs(2));
        return;
    }

    while let Ok(request) = codec.read_json::<_, ExtensionRequest>(&mut stdin) {
        let request_id = request.request_id().to_string();
        if mode == "unknown" {
            let response = ExtensionResponse::success(
                "unsolicited-response",
                ToolsListResult { tools: Vec::new() },
            );
            let _ = write_response(&codec, &mut stdout, response);
            std::thread::sleep(Duration::from_secs(2));
            return;
        }
        if mode == "late" {
            std::thread::sleep(Duration::from_millis(50));
            let response = ExtensionResponse::success(
                request_id.clone(),
                ToolsListResult { tools: Vec::new() },
            );
            if write_response(&codec, &mut stdout, response).is_err() {
                return;
            }
            continue;
        }
        let response = match request.method() {
            ExtensionMethod::Initialize => ExtensionResponse::success(
                request_id,
                InitializeResult {
                    protocol: extension_protocol::PROTOCOL_NAME.to_string(),
                    version: extension_protocol::PROTOCOL_VERSION,
                    capabilities: vec![
                        ExtensionCapability::Cancel,
                        ExtensionCapability::StructuredErrors,
                        ExtensionCapability::Hooks,
                    ],
                },
            ),
            ExtensionMethod::ToolsList => ExtensionResponse::success(
                request_id,
                ToolsListResult {
                    tools: vec![ToolDescriptor {
                        name: "echo".to_string(),
                        description: Some("stdio fixture echo".to_string()),
                        input_schema: Some(json!({
                            "type": "object",
                            "properties": { "value": { "type": "string" } },
                            "required": ["value"],
                            "additionalProperties": false
                        })),
                    }],
                },
            ),
            ExtensionMethod::ToolsExecute => ExtensionResponse::success(
                request_id,
                ToolExecuteResult {
                    content: vec![ToolContent::Text("stdio-ok".to_string())],
                    is_error: false,
                },
            ),
            ExtensionMethod::ToolsCancel => ExtensionResponse::success(
                request_id,
                extension_protocol::ToolCancelResult { accepted: true },
            ),
            ExtensionMethod::HooksList => ExtensionResponse::success(
                request_id,
                HooksListResult {
                    hooks: vec![HookDescriptor {
                        hook_id: "stdio-hook".to_string(),
                        phase: HookPhase::BeforeTurn,
                        priority: 0,
                    }],
                },
            ),
            ExtensionMethod::HooksBeforeTurn => {
                let params = request
                    .decode_params::<BeforeTurnHookParams>()
                    .expect("fixture hook params should decode");
                let mut items = params.items;
                if let Some(first) = items.first().cloned() {
                    items.push(first);
                }
                ExtensionResponse::success(request_id, BeforeTurnHookResult::Continue { items })
            }
            ExtensionMethod::HooksCancel => {
                ExtensionResponse::success(request_id, HookCancelResult { accepted: true })
            }
            ExtensionMethod::HooksBeforeToolExecute | ExtensionMethod::HooksAfterToolResult => {
                Ok(ExtensionResponse::failure(
                    request_id,
                    extension_protocol::ExtensionError::new(
                        extension_protocol::ExtensionErrorCode::CapabilityDenied,
                        "hooks are unavailable",
                        false,
                    ),
                ))
            }
            ExtensionMethod::Shutdown => {
                let response = ExtensionResponse::success(
                    request_id,
                    extension_protocol::ShutdownResult { drained: true },
                );
                let _ = write_response(&codec, &mut stdout, response);
                break;
            }
        };
        if write_response(&codec, &mut stdout, response).is_err() {
            break;
        }
    }
}

fn write_response<W: Write>(
    codec: &FrameCodec,
    writer: &mut W,
    response: Result<ExtensionResponse, extension_protocol::ProtocolEncodeError>,
) -> io::Result<()> {
    let response = response.map_err(|_| io::Error::other("fixture response encoding failed"))?;
    codec
        .write_json(writer, &response)
        .map_err(|_| io::Error::other("fixture response write failed"))
}
