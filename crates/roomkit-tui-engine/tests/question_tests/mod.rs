//! Questions an agent asks the person: the kit's human-input request, shown
//! to the views of its room and answered from one of them (RFC §9.3).

use roomkit::human_input::PendingInputEvent;
use roomkit::model::RoomId;
use roomkit_ai::{HumanInputToolHandler, HumanInputToolHandlerOptions};
use roomkit_tui_engine::QuestionRequest;

use super::*;

const ASK: &str = r#"{"questions":[{"header":"Database","question":"Which database holds the sessions?","options":[{"label":"SQLite","description":"One file"},{"label":"Postgres","description":"Shared"}]}]}"#;

/// A room whose agent serves `ask_user` through the kit's human input, and
/// an `ON_USER_INPUT_REQUIRED` hook that shows its questions to the views,
/// as an application does.
async fn asking_room<S: ConversationStore>(
    kit: &RoomKit<S>,
    provider: ScriptedProvider,
) -> ConversationChannel {
    let ask = AiTool {
        name: "ask_user".to_owned(),
        description: "Ask the person".to_owned(),
        parameters: json!({"type": "object", "properties": {"questions": {"type": "array"}}}),
        tags: Vec::new(),
        changes_state: false,
        defer_loading: false,
    };
    let options = HumanInputToolHandlerOptions {
        tool_definitions: vec![ask],
        ..Default::default()
    };
    let human = HumanInputToolHandler::new(["ask_user"], options).unwrap();
    let agent = config().agent;
    let options = AiChannelOptions {
        human_input_handler: Some(human),
        ..Default::default()
    };
    kit.register_channel(AiChannel::new(agent.clone(), provider, options).unwrap())
        .unwrap();
    let channel = ConversationChannel::new(ChannelId::new("console").unwrap());
    kit.register_channel(channel.clone()).unwrap();
    let room = config().room;
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [channel.id(), &agent] {
        kit.attach_channel(&room, id, Default::default())
            .await
            .unwrap();
    }
    let shown = channel.clone();
    kit.hook(
        roomkit::HookTrigger::OnUserInputRequired,
        move |event: PendingInputEvent, _| {
            let shown = shown.clone();
            async move {
                let request = QuestionRequest::new(event.pending_id.clone(), questions(&event))
                    .with_tool_call(event.tool_call_id.clone());
                let room = RoomId::new(&event.room_id).unwrap();
                let agent = ChannelId::new(&event.channel_id).unwrap();
                assert!(shown.ask_question(&room, &agent, request));
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("question"),
    )
    .unwrap();
    channel
}

/// The questions an `ask_user` call carries, as the application reads them.
fn questions(event: &PendingInputEvent) -> Vec<QuestionItem> {
    let asked = event.arguments["questions"].as_array().unwrap();
    asked
        .iter()
        .map(|question| {
            let options = question["options"]
                .as_array()
                .unwrap()
                .iter()
                .map(|option| {
                    QuestionOption::new(
                        option["label"].as_str().unwrap(),
                        option["description"].as_str().unwrap_or_default(),
                    )
                });
            QuestionItem::new(
                question["header"].as_str().unwrap(),
                question["question"].as_str().unwrap(),
                options.collect(),
            )
        })
        .collect()
}

fn answer(question: &Question, answers: Option<Vec<QuestionAnswer>>) -> Request {
    Request::new(
        RequestId::new(2),
        "answer",
        "room",
        Command::AnswerQuestion {
            question: question.id.clone(),
            answers,
        },
    )
}

/// The events until the first question is asked, and that question.
async fn until_asked(client: &Client) -> (Vec<Event>, Question) {
    let mut events = Vec::new();
    loop {
        let event = next(client).await;
        if let Event::QuestionAsked { question, .. } = &event {
            let question = question.clone();
            events.push(event);
            return (events, question);
        }
        events.push(event);
    }
}

fn rounds() -> ScriptedProvider {
    ScriptedProvider::new([
        ScriptedRound::calling("", [("ask1", "ask_user", ASK)]),
        ScriptedRound::answering("SQLite it is."),
    ])
}

fn resolved(events: &[Event], question: &Question) -> bool {
    events.iter().any(
        |event| matches!(event, Event::QuestionResolved { question: id, .. } if *id == question.id),
    )
}

/// What the agent read back for its call `ask1`.
fn read_back(provider: &ScriptedProvider) -> String {
    let request = provider
        .requests()
        .into_iter()
        .nth(1)
        .expect("a second request");
    format!("{:?}", request.messages)
}

#[tokio::test]
async fn a_question_reaches_the_view_and_its_answer_the_agent() {
    let kit = RoomKit::new(Default::default());
    let provider = rounds();
    let channel = asking_room(&kit, provider.clone()).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("ask")).unwrap();

    let (_, question) = until_asked(&client).await;
    assert_eq!(question.items.len(), 1);
    assert_eq!(question.items[0].header, "Database");
    assert_eq!(question.items[0].options[1].label, "Postgres");
    assert_eq!(question.actor.as_str(), "agent");
    let picked = QuestionAnswer::new(vec!["SQLite".to_owned()]);
    client
        .try_send(answer(&question, Some(vec![picked])))
        .unwrap();
    let events = collect_operation(&client).await;

    assert!(
        events.iter().any(
            |event| matches!(event, Event::Reply { request, error: None } if *request == RequestId::new(2))
        ),
        "{events:?}"
    );
    assert!(resolved(&events, &question), "{events:?}");
    assert_operation_ended(&events, RunOutcome::Completed);
    assert!(
        read_back(&provider).contains("Database: SQLite"),
        "{}",
        read_back(&provider)
    );
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn a_question_closed_unanswered_tells_the_agent_to_go_on() {
    let kit = RoomKit::new(Default::default());
    let provider = rounds();
    let channel = asking_room(&kit, provider.clone()).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("ask")).unwrap();

    let (_, question) = until_asked(&client).await;
    client.try_send(answer(&question, None)).unwrap();
    let events = collect_operation(&client).await;

    assert!(resolved(&events, &question), "{events:?}");
    let read = read_back(&provider);
    assert!(
        read.contains("closed the question without answering"),
        "{read}"
    );
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn answers_that_do_not_fit_are_refused_and_the_question_stays() {
    let kit = RoomKit::new(Default::default());
    let channel = asking_room(&kit, rounds()).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("ask")).unwrap();

    let (_, question) = until_asked(&client).await;
    let unknown = QuestionAnswer::new(vec!["Redis".to_owned()]);
    client
        .try_send(answer(&question, Some(vec![unknown])))
        .unwrap();
    let refused = loop {
        if let Event::Reply { request, error } = next(&client).await
            && request == RequestId::new(2)
        {
            break error;
        }
    };
    assert!(refused.is_some(), "an option not offered is refused");
    // Still asked: the right answer is taken.
    let picked = QuestionAnswer::new(vec!["Postgres".to_owned()]);
    client
        .try_send(answer(&question, Some(vec![picked])))
        .unwrap();
    let events = collect_operation(&client).await;
    assert!(resolved(&events, &question), "{events:?}");
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn a_cut_turn_withdraws_its_question() {
    let kit = RoomKit::new(Default::default());
    let channel = asking_room(&kit, rounds()).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("ask")).unwrap();
    let (events, question) = until_asked(&client).await;

    let run = events
        .iter()
        .find_map(|event| match event {
            Event::RunStarted { run, .. } => Some(run.clone()),
            _ => None,
        })
        .expect("the asking run");
    let stop = Request::new(
        RequestId::new(3),
        "stop",
        "room",
        Command::Interrupt { run },
    );
    client.try_send(stop).unwrap();
    let events = collect_operation(&client).await;
    assert!(resolved(&events, &question), "{events:?}");
    // Answering it now is refused: it is no longer asked.
    let late = QuestionAnswer::new(vec!["SQLite".to_owned()]);
    client
        .try_send(answer(&question, Some(vec![late])))
        .unwrap();
    let refused = loop {
        if let Event::Reply { request, error } = next(&client).await
            && request == RequestId::new(2)
        {
            break error;
        }
    };
    assert!(refused.is_some());
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

fn assert_operation_ended(events: &[Event], outcome: RunOutcome) {
    assert!(
        matches!(events.last(), Some(Event::OperationFinished { outcome: actual, .. }) if *actual == outcome),
        "{events:?}"
    );
}
