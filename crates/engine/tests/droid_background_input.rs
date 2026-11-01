//! Child input stays answerable after the native parent turn completes.
#![cfg(unix)]

use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
use zeron_doc::MessagePart;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::DroidHarness;
use zeron_proto::{
    AccessMode, HarnessId, RunRequest, SandboxLevel, SessionStatus, UserInputAnswer,
};

const PEER: &str = r#"#!/usr/bin/env python3
import json,sys
settings={"modelId":"fixture","reasoningEffort":"low","specModeModelId":"fixture","specModeReasoningEffort":"low","autonomyLevel":"high","availableAutonomyLevels":["off","low","medium","high"],"interactionMode":"auto","toolExecutionMode":"direct_only"}
children=["11111111-1111-4111-8111-111111111111","22222222-2222-4222-8222-222222222222"]
def send(m):
 m.update(jsonrpc="2.0",factoryApiVersion="1.0.0")
 print(json.dumps(m),flush=True)
def notify(n):
 send({"type":"notification","method":"droid.session_notification","params":{"notification":n}})
def done():
 notify({"type":"agent_turn_completed","reason":"completed"})
def ask(i,permission=False):
 child=children[i]
 params={"associatedSessionIds":["saved-droid",child]}
 if permission:
  method="droid.request_permission"
  params.update(toolUses=[{"toolUse":{"id":"child-exec","name":"Execute","input":{"command":"printf child"}},"confirmationType":"exec","details":{"fullCommand":"printf child"}}],options=[{"value":"proceed_once"},{"value":"cancel"}])
 else:
  method="droid.ask_user"
  params["questions"]=[{"index":1,"topic":"Child "+str(i),"question":"Choose for child "+str(i),"options":["Yes","No"],"multiSelect":False}]
 send({"type":"request","id":"child-"+str(i),"method":method,"params":params})
for line in sys.stdin:
 m=json.loads(line);p=m.get("params",{});method=m.get("method")
 if m["type"]=="response":
  with open("child-answers.jsonl","a") as answers:answers.write(json.dumps(m)+"\n")
  if m["id"]=="parent-input":continue
  i=int(m["id"].split("-")[1])
  result=m.get("result",{})
  value=result.get("selectedOption") or ("answered" if result.get("answers") else "cancel")
  notify({"type":"assistant_text_delta","textDelta":"child-result:"+value})
  notify({"type":"child_session_settled","childSessionId":children[i],"status":"completed"})
  done()
 elif method in ["droid.initialize_session","droid.load_session"]:
  settings.update({k:v for k,v in p.items() if k in settings})
  send({"type":"response","id":m["id"],"result":{"sessionId":"saved-droid","settings":settings,"availableModels":[]}})
 elif method=="droid.update_session_settings":
  settings.update(p)
  notify({"type":"settings_updated","requestId":m["id"],"settings":settings})
  send({"type":"response","id":m["id"],"result":{}})
 elif method=="droid.add_user_message":
  send({"type":"response","id":m["id"],"result":{}})
  notify({"type":"create_message","requestId":m["id"],"message":{"role":"user"}})
  prompt=p["text"]
  if prompt=="hold":
   notify({"type":"assistant_text_delta","textDelta":"parent holding"})
   continue
  if prompt=="settle":
   notify({"type":"child_session_settled","childSessionId":children[0],"status":"completed"})
   done()
   continue
  count=2 if prompt=="multi" else 1
  for i in range(count):
   notify({"type":"tool_call","toolUse":{"id":"spawn-"+str(i),"name":"Task","input":{"description":"input child"}}})
   notify({"type":"child_session_available","childSessionId":children[i],"toolUseId":"spawn-"+str(i)})
  if prompt in ["before","mixed"]:ask(0)
  if prompt=="mixed":
   send({"type":"request","id":"parent-input","method":"droid.ask_user","params":{"questions":[{"index":1,"topic":"Parent","question":"Parent question","options":["Yes","No"],"multiSelect":False}]}})
  notify({"type":"assistant_text_delta","textDelta":"parent finished"})
  done()
  if prompt not in ["before","mixed"]:
   for i in range(count):ask(i,prompt=="permission")
 elif method=="droid.interrupt_session":
  send({"type":"response","id":m["id"],"result":{}})
  done()
"#;

fn request(cwd: &std::path::Path, prompt: &str, access: AccessMode) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access,
        plan: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
        env: [("FACTORY_HOME_OVERRIDE".into(), cwd.display().to_string())].into(),
    }
}

fn engine(dir: &std::path::Path) -> EngineCore {
    let fixture = dir.join("droid.py");
    std::fs::write(&fixture, PEER).unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o755)).unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(
        DroidHarness::new()
            .with_executable(fixture)
            .with_graces(Duration::from_millis(50), Duration::from_millis(100)),
    ));
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Droid, None).unwrap()
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("background input lifecycle did not settle");
}

fn inputs(core: &EngineCore, chat: &str) -> Vec<(String, bool)> {
    core.doc_host
        .open(chat)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .flat_map(|entry| entry.parts)
        .filter_map(|part| match part {
            MessagePart::Input {
                request_id,
                resolved,
                ..
            } => Some((request_id, resolved)),
            _ => None,
        })
        .collect()
}

fn status(core: &EngineCore, chat: &str) -> Option<SessionStatus> {
    core.sessions
        .watch_sessions()
        .borrow()
        .iter()
        .find(|s| s.chat_id == chat)
        .map(|s| s.status)
}

fn approve(core: &EngineCore, chat: &str, id: &str) {
    let answers = core
        .doc_host
        .open(chat)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .flat_map(|entry| entry.parts)
        .find_map(|part| match part {
            MessagePart::Input {
                request_id,
                questions,
                ..
            } if request_id == id => Some(
                questions
                    .into_iter()
                    .map(|question| UserInputAnswer {
                        question_id: question.id,
                        labels: vec!["Yes".into()],
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap();
    assert!(core.sessions.respond_input(chat, id, answers).unwrap());
}

#[tokio::test]
async fn child_input_survives_parent_completion_and_answers_without_reopening_it() {
    for prompt in ["before", "after", "permission", "mixed"] {
        let dir = tempfile::tempdir().unwrap();
        let core = engine(dir.path());
        let chat = "background-input";
        core.sessions
            .dispatch(
                chat,
                HarnessId::Droid,
                request(dir.path(), prompt, AccessMode::Supervised),
                None,
            )
            .await
            .unwrap();
        wait_for(|| {
            inputs(&core, chat).iter().any(|(_, resolved)| !resolved)
                && !core.sessions.turn_in_flight(chat)
        })
        .await;
        assert_eq!(status(&core, chat), Some(SessionStatus::AwaitingInput));
        assert!(
            core.sessions
                .watch_sessions()
                .borrow()
                .iter()
                .find(|session| session.chat_id == chat)
                .unwrap()
                .started_at
                .is_none(),
            "child input must not retain the finished parent's elapsed timer"
        );
        let id = inputs(&core, chat)
            .into_iter()
            .find(|(_, resolved)| !resolved)
            .unwrap()
            .0;
        approve(&core, chat, &id);
        assert!(
            !core.sessions.respond_input(chat, &id, vec![]).unwrap(),
            "answers are single-use"
        );
        wait_for(|| {
            inputs(&core, chat).iter().all(|(_, resolved)| *resolved)
                && status(&core, chat) == Some(SessionStatus::Idle)
                && !core.sessions.holds_background_work(chat)
        })
        .await;
        let entries = core
            .doc_host
            .open(chat)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap();
        assert!(!entries.iter().flat_map(|entry| &entry.parts).any(|part|
            matches!(part, MessagePart::Text { text, .. } if text.contains("child-result:cancel"))));
        let replies = std::fs::read_to_string(dir.path().join("child-answers.jsonl")).unwrap();
        let replies: Vec<serde_json::Value> = replies
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let reply = replies
            .iter()
            .find(|reply| reply["id"] == "child-0")
            .unwrap();
        assert_eq!(replies.len(), if prompt == "mixed" { 2 } else { 1 });
        if prompt == "mixed" {
            let parent = replies
                .iter()
                .find(|reply| reply["id"] == "parent-input")
                .unwrap();
            assert_eq!(parent["result"]["cancelled"], true);
        }
        if prompt == "permission" {
            assert_eq!(reply["result"]["selectedOption"], "proceed_once");
        } else {
            assert_eq!(reply["result"]["answers"][0]["answer"], "Yes");
        }
        core.shutdown().await;
    }
}

#[tokio::test]
async fn one_child_finishing_keeps_the_other_child_question_open() {
    let dir = tempfile::tempdir().unwrap();
    let core = engine(dir.path());
    let chat = "two-inputs";
    core.sessions
        .dispatch(
            chat,
            HarnessId::Droid,
            request(dir.path(), "multi", AccessMode::FullAccess),
            None,
        )
        .await
        .unwrap();
    wait_for(|| {
        inputs(&core, chat)
            .iter()
            .filter(|(_, resolved)| !resolved)
            .count()
            == 2
            && !core.sessions.turn_in_flight(chat)
    })
    .await;
    for remaining in [2, 1] {
        let id = inputs(&core, chat)
            .into_iter()
            .find(|(_, resolved)| !resolved)
            .unwrap()
            .0;
        approve(&core, chat, &id);
        wait_for(|| {
            inputs(&core, chat)
                .iter()
                .filter(|(_, resolved)| !resolved)
                .count()
                == remaining - 1
                && !core.sessions.turn_in_flight(chat)
        })
        .await;
        assert_eq!(
            status(&core, chat),
            Some(if remaining == 2 {
                SessionStatus::AwaitingInput
            } else {
                SessionStatus::Idle
            })
        );
    }
    core.shutdown().await;
}

#[tokio::test]
async fn teardown_closes_child_questions_and_rejects_late_answers() {
    let dir = tempfile::tempdir().unwrap();
    let core = engine(dir.path());
    let chat = "cancel-child";
    core.sessions
        .dispatch(
            chat,
            HarnessId::Droid,
            request(dir.path(), "after", AccessMode::FullAccess),
            None,
        )
        .await
        .unwrap();
    wait_for(|| {
        inputs(&core, chat).iter().any(|(_, resolved)| !resolved)
            && !core.sessions.turn_in_flight(chat)
    })
    .await;
    let id = inputs(&core, chat)[0].0.clone();
    assert!(core.sessions.terminate(chat).await.unwrap());
    wait_for(|| {
        inputs(&core, chat).iter().all(|(_, resolved)| *resolved)
            && status(&core, chat) == Some(SessionStatus::Idle)
    })
    .await;
    assert!(!core.sessions.respond_input(chat, &id, vec![]).unwrap());
    core.shutdown().await;
}

#[tokio::test]
async fn stopping_a_new_parent_turn_keeps_the_existing_child_question() {
    let dir = tempfile::tempdir().unwrap();
    let core = engine(dir.path());
    let chat = "parent-stop";
    core.sessions
        .dispatch(
            chat,
            HarnessId::Droid,
            request(dir.path(), "after", AccessMode::FullAccess),
            None,
        )
        .await
        .unwrap();
    wait_for(|| {
        inputs(&core, chat).iter().any(|(_, resolved)| !resolved)
            && !core.sessions.turn_in_flight(chat)
    })
    .await;
    let id = inputs(&core, chat)[0].0.clone();
    core.sessions
        .dispatch(
            chat,
            HarnessId::Droid,
            request(dir.path(), "hold", AccessMode::FullAccess),
            None,
        )
        .await
        .unwrap();
    wait_for(|| core.sessions.turn_in_flight(chat)).await;
    assert!(core.sessions.stop_turn(chat).await.unwrap());
    assert_eq!(inputs(&core, chat), vec![(id.clone(), false)]);
    assert_eq!(status(&core, chat), Some(SessionStatus::AwaitingInput));
    approve(&core, chat, &id);
    wait_for(|| {
        inputs(&core, chat).iter().all(|(_, resolved)| *resolved)
            && status(&core, chat) == Some(SessionStatus::Idle)
    })
    .await;
    core.shutdown().await;
}

#[tokio::test]
async fn child_settlement_closes_its_input_before_transcript_drain() {
    let dir = tempfile::tempdir().unwrap();
    let core = engine(dir.path());
    let chat = "child-settle";
    core.sessions
        .dispatch(
            chat,
            HarnessId::Droid,
            request(dir.path(), "after", AccessMode::FullAccess),
            None,
        )
        .await
        .unwrap();
    wait_for(|| {
        inputs(&core, chat).iter().any(|(_, resolved)| !resolved)
            && !core.sessions.turn_in_flight(chat)
    })
    .await;
    let id = inputs(&core, chat)[0].0.clone();
    core.sessions
        .dispatch(
            chat,
            HarnessId::Droid,
            request(dir.path(), "settle", AccessMode::FullAccess),
            None,
        )
        .await
        .unwrap();
    wait_for(|| {
        inputs(&core, chat).iter().all(|(_, resolved)| *resolved)
            && !core.sessions.holds_background_work(chat)
            && !core.sessions.turn_in_flight(chat)
    })
    .await;
    assert!(!core.sessions.respond_input(chat, &id, vec![]).unwrap());
    core.shutdown().await;
}
