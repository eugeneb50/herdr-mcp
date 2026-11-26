#![allow(clippy::unwrap_used, reason = "fixture operations must succeed")]

use super::*;
use herdr_mcp_api::settings::Value as Setting;
use tempfile::TempDir;
use tokio::time::advance;

const LABEL: &str = "worker";
const MAILBOX: &str = "mail-worker";
const PANE: &str = "w1:p7";
const CONFIRM: u64 = 500;
const LEASE: u64 = 1_000;
const WAIT: u64 = 60_000;

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Panes,
    Prompt(String, String),
    Enter(String),
}

#[derive(Default)]
struct FakeMailHerdr {
    panes: Mutex<Vec<MailPane>>,
    calls: Mutex<Vec<Call>>,
    fail_prompt: bool,
    fail_enter: bool,
}

impl MailHerdr for FakeMailHerdr {
    fn panes(&self) -> BoxFuture<'_, Result<Vec<MailPane>>> {
        Box::pin(async {
            lock(&self.calls).push(Call::Panes);
            Ok(lock(&self.panes).clone())
        })
    }

    fn prompt<'a>(&'a self, pane: &'a str, text: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            lock(&self.calls).push(Call::Prompt(pane.into(), text.into()));
            anyhow::ensure!(!self.fail_prompt, "scripted prompt failure");
            Ok(())
        })
    }

    fn enter<'a>(&'a self, pane: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            lock(&self.calls).push(Call::Enter(pane.into()));
            anyhow::ensure!(!self.fail_enter, "scripted Enter failure");
            Ok(())
        })
    }
}

impl FakeMailHerdr {
    fn pane(&self, status: &str, seq: i64) {
        *lock(&self.panes) = vec![MailPane {
            pane_id: PANE.into(),
            label: LABEL.into(),
            status: status.into(),
            seq: Some(seq),
        }];
    }

    fn prompts(&self) -> usize {
        lock(&self.calls)
            .iter()
            .filter(|call| matches!(call, Call::Prompt(..)))
            .count()
    }

    fn enters(&self) -> usize {
        lock(&self.calls)
            .iter()
            .filter(|call| matches!(call, Call::Enter(_)))
            .count()
    }
}

struct Fixture {
    _dir: TempDir,
    server: HerdrMcpServer,
}

impl Fixture {
    async fn new(enabled: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let persistence = crate::Persistence::new(dir.path().to_owned());
        persistence.init().await.unwrap();
        let server = HerdrMcpServer::new(persistence, crate::AgentRegistry::new());
        let fixture = Self { _dir: dir, server };
        fixture.labels(&[LABEL]);
        if enabled {
            fixture.setting(MAIL_ENABLED, Setting::Bool(true));
        }
        for (key, ms) in [
            (MAIL_CONSUMER_LEASE_MS, LEASE),
            (MAIL_REPLY_WAIT_MS, WAIT),
            (MAIL_SUBMIT_CONFIRM_MS, CONFIRM),
        ] {
            fixture.setting(key, Setting::Number(ms));
        }
        fixture
    }

    fn setting(&self, key: &str, value: Setting) {
        crate::write(&self.server.settings).set(key, value).unwrap();
    }

    fn labels(&self, labels: &[&str]) {
        self.setting(
            MAIL_LABELS,
            Setting::List(labels.iter().map(|label| (*label).into()).collect()),
        );
    }

    fn path(&self) -> std::path::PathBuf {
        self.server.persistence.data_dir().join("queue.db")
    }

    fn db(&self) -> Queue {
        Queue::open(self.path()).unwrap()
    }

    fn now(&self) -> i64 {
        self.server.queues.now()
    }

    fn push(&self, capsule: &Capsule) -> String {
        push(&mut self.db(), capsule, None, self.now())
    }

    fn item(&self, id: &str) -> Item {
        self.db().get(id).unwrap()
    }

    async fn tick(&self, postman: &Postman) -> Vec<(String, Step)> {
        self.server
            .queues
            .mail_tick(&self.server, postman)
            .await
            .unwrap()
    }

    async fn step(&self, postman: &Postman, expected: Step) {
        assert_eq!(self.tick(postman).await, [(LABEL.to_owned(), expected)]);
    }
}

fn push(db: &mut Queue, capsule: &Capsule, dedupe_key: Option<String>, at: i64) -> String {
    let payload = serde_json::to_value(capsule).unwrap();
    let pushed = db
        .push(
            Push {
                queue: format!("mail-{}", capsule.to),
                payload: payload.as_object().unwrap().clone(),
                dedupe_key,
                coalesce_key: None,
                requester: None,
                max_attempts: 3,
                max_depth: None,
            },
            at,
        )
        .unwrap();
    assert!(matches!(pushed, Pushed::New(_)), "{pushed:?}");
    match pushed {
        Pushed::New(item) | Pushed::Deduped(item) | Pushed::Coalesced(item) => item.id,
    }
}

fn task(id: &str) -> Capsule {
    Capsule {
        v: 2,
        id: id.into(),
        task: "test-task".into(),
        from: "lead".into(),
        to: LABEL.into(),
        status: "task".into(),
        summary: "Run the contract fixture".into(),
        reply_to: Some("return".into()),
        ..Capsule::default()
    }
}

fn reply(original: &Capsule, delivery: &str) -> Capsule {
    Capsule {
        v: 2,
        id: format!("reply-{delivery}"),
        task: original.task.clone(),
        from: original.to.clone(),
        to: "return".into(),
        status: "done".into(),
        re: Some(original.id.clone()),
        delivery: Some(delivery.into()),
        ..Capsule::default()
    }
}

fn fake() -> (Arc<FakeMailHerdr>, Postman) {
    let herdr = Arc::new(FakeMailHerdr::default());
    herdr.pane("idle", 1);
    let postman = Postman::new(herdr.clone());
    (herdr, postman)
}

async fn elapsed(ms: u64) {
    advance(Duration::from_millis(ms)).await;
}

#[tokio::test(start_paused = true)]
async fn default_disabled_never_calls_herdr() {
    let f = Fixture::new(false).await;
    let (herdr, postman) = fake();
    let id = f.push(&task("one"));
    assert!(f.tick(&postman).await.is_empty());
    assert!(lock(&herdr.calls).is_empty());
    assert_eq!(f.item(&id).status, ItemStatus::Queued);
}

#[tokio::test(start_paused = true)]
async fn postman_never_types_to_lead() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    // The existing ready pane fixture, relabelled to the mailbox `capsule watch` owns.
    lock(&herdr.panes)[0].label = "lead".into();
    f.labels(&["lead"]);
    let id = f.push(&Capsule {
        to: "lead".into(),
        ..task("lead-capsule")
    });

    assert!(f.tick(&postman).await.is_empty());
    assert_eq!(herdr.prompts(), 0);
    assert_eq!(herdr.enters(), 0);
    assert_eq!(f.item(&id).status, ItemStatus::Queued);
    // No consumer lease was taken: `capsule watch` can own the mailbox.
    assert!(
        f.db()
            .claim_consumer("mail-lead", "capsule-watch", LEASE, f.now())
            .unwrap()
            .is_some()
    );
    assert!(
        f.db()
            .list(Some("mail-lead"), None, 10)
            .unwrap()
            .iter()
            .any(|item| item.id == id)
    );
}

#[tokio::test(start_paused = true)]
async fn second_task_waits_for_the_first_delivery_reply() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let first = task("one");
    let one = f.push(&first);
    let two = f.push(&task("two"));

    f.tick(&postman).await;
    let delivery = f.item(&one).delivery.unwrap();
    assert_eq!(herdr.prompts(), 1);
    {
        let calls = lock(&herdr.calls);
        assert!(
            matches!(&calls[1], Call::Prompt(pane, text) if pane == PANE && text.contains(&delivery))
        );
    }
    for (i, status) in ["working", "blocked"].iter().enumerate() {
        herdr.pane(status, i as i64 + 2);
        f.step(&postman, Step::Waiting).await;
        assert_eq!(herdr.prompts(), 1);
        assert_eq!(f.item(&two).status, ItemStatus::Queued);
    }
    let answer = f.push(&reply(&first, &delivery));
    f.step(&postman, Step::Replied(answer)).await;
    assert_eq!(f.item(&one).status, ItemStatus::Done);
    f.step(&postman, Step::Busy).await;
    assert_eq!(herdr.prompts(), 1);
    herdr.pane("idle", 4);
    f.tick(&postman).await;
    assert_eq!(f.item(&two).status, ItemStatus::Running);
    assert_eq!(herdr.prompts(), 2);
}

#[tokio::test(start_paused = true)]
async fn idle_flicker_neither_finishes_nor_admits_a_task() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let one = f.push(&task("one"));
    let two = f.push(&task("two"));
    f.tick(&postman).await;
    herdr.pane("working", 2);
    f.step(&postman, Step::Waiting).await;
    herdr.pane("idle", 3);
    f.step(&postman, Step::Waiting).await;
    assert_eq!(f.item(&one).status, ItemStatus::Running);
    assert_eq!(f.item(&two).status, ItemStatus::Queued);
    assert_eq!(herdr.prompts(), 1);
}

#[tokio::test(start_paused = true)]
async fn enter_is_one_shot_and_only_when_the_turn_never_started() {
    for (starts, fail, enters) in [(false, false, 1), (true, false, 0), (false, true, 1)] {
        let f = Fixture::new(true).await;
        let herdr = Arc::new(FakeMailHerdr {
            fail_enter: fail,
            ..Default::default()
        });
        herdr.pane("idle", 1);
        let postman = Postman::new(herdr.clone());
        f.push(&task("one"));
        f.tick(&postman).await;
        if starts {
            herdr.pane("working", 2);
        }
        elapsed(CONFIRM - 1).await;
        f.step(&postman, Step::Waiting).await;
        assert_eq!(herdr.enters(), 0);
        elapsed(1).await;
        let expected = if starts || fail {
            Step::Waiting
        } else {
            Step::Nudged
        };
        f.step(&postman, expected).await;
        assert_eq!(herdr.enters(), enters);
        herdr.pane("idle", 1);
        elapsed(CONFIRM).await;
        f.step(&postman, Step::Waiting).await;
        assert_eq!(herdr.enters(), enters);
    }
}

#[tokio::test(start_paused = true)]
async fn reply_for_previous_delivery_cannot_finish_the_current_one() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let capsule = task("one");
    let id = f.push(&capsule);
    let mut db = f.db();
    let (_, old_lease) = db.claim(MAILBOX, LEASE, f.now()).unwrap().unwrap();
    db.set_delivery(&old_lease, "previous", json!({"submitted": false}), f.now())
        .unwrap();
    f.tick(&postman).await;
    let current = f.item(&id).delivery.unwrap();
    assert_ne!(current, "previous");

    f.push(&reply(&capsule, "previous"));
    f.step(&postman, Step::Waiting).await;
    assert_eq!(f.item(&id).status, ItemStatus::Running);
    let answer = f.push(&reply(&capsule, &current));
    f.step(&postman, Step::Replied(answer)).await;
    assert_eq!(f.item(&id).status, ItemStatus::Done);
    assert_eq!(herdr.prompts(), 1);
}

#[tokio::test(start_paused = true)]
async fn sees_reply_committed_by_an_independent_connection() {
    let f = Fixture::new(true).await;
    let (_, postman) = fake();
    let capsule = task("one");
    let id = f.push(&capsule);
    f.tick(&postman).await;
    let delivery = f.item(&id).delivery.unwrap();

    let answer = push(
        &mut Queue::open(f.path()).unwrap(),
        &reply(&capsule, &delivery),
        None,
        f.now(),
    );
    f.step(&postman, Step::Replied(answer.clone())).await;
    assert_eq!(
        f.item(&id).result.unwrap(),
        json!({"reply": answer, "delivery": delivery})
    );
}

#[tokio::test(start_paused = true)]
async fn reply_deadline_retries_after_notice_enqueue_rolls_back() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let capsule = task("one");
    let id = f.push(&capsule);
    f.tick(&postman).await;

    let connection = rusqlite::Connection::open(f.path()).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER interrupt_notice BEFORE INSERT ON queue_items
             WHEN NEW.queue = 'mail-return'
             BEGIN SELECT RAISE(ABORT, 'interrupted notice'); END;",
        )
        .unwrap();

    herdr.pane("blocked", 2);
    elapsed(WAIT - 1).await;
    f.step(&postman, Step::Waiting).await;
    elapsed(1).await;
    f.tick(&postman).await;

    assert_eq!(f.item(&id).status, ItemStatus::Running);
    assert!(
        f.db()
            .item(&id)
            .unwrap()
            .unwrap()
            .1
            .iter()
            .all(|event| event.kind != "dead")
    );
    assert!(
        f.db()
            .list(Some("mail-return"), None, 10)
            .unwrap()
            .is_empty()
    );

    connection
        .execute_batch("DROP TRIGGER interrupt_notice")
        .unwrap();
    f.tick(&postman).await;
    f.tick(&postman).await;

    assert_eq!(f.item(&id).status, ItemStatus::Dead);
    let notices = f.db().list(Some("mail-return"), None, 10).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].payload["status"], "fail");
    assert_eq!(notices[0].payload["re"], capsule.id);
    assert!(!notices[0].payload.contains_key("delivery"));
    assert!(f.db().list(Some("mail-lead"), None, 10).unwrap().is_empty());
    assert_eq!(herdr.prompts(), 1);
    herdr_queue::run_audit(&f.path(), None, None).unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancelled_flight_emits_no_dead_notice() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let id = f.push(&task("one"));
    f.tick(&postman).await;
    herdr.pane("blocked", 2);

    // Cancel just after the heartbeat's fence check, before timeout settlement.
    // This places the cancellation at the same SQLite write boundary each run.
    let connection = rusqlite::Connection::open(f.path()).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER cancel_after_heartbeat AFTER UPDATE OF lease_until ON queue_items
             WHEN NEW.queue = 'mail-worker' AND NEW.status = 'running'
             BEGIN
                 UPDATE queue_items SET status = 'cancelled', lease_until = NULL
                 WHERE id = NEW.id;
                 INSERT INTO queue_events (item_id, seq, at, kind)
                 SELECT NEW.id, MAX(seq) + 1, NEW.lease_until, 'cancelled'
                 FROM queue_events WHERE item_id = NEW.id;
             END;",
        )
        .unwrap();

    elapsed(WAIT).await;
    f.step(&postman, Step::Empty).await;
    assert_eq!(f.item(&id).status, ItemStatus::Cancelled);
    assert!(
        f.db()
            .list(Some("mail-return"), None, 10)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(start_paused = true)]
async fn restart_resends_only_a_delivery_without_submission_record() {
    for submitted in [false, true] {
        let f = Fixture::new(true).await;
        let (herdr, postman) = fake();
        let capsule = task("one");
        let id = f.push(&capsule);
        let mut db = f.db();
        let consumer = db
            .claim_consumer(MAILBOX, "crashed", LEASE, f.now())
            .unwrap()
            .unwrap();
        let Fenced::Applied(Some((_, lease))) = db.claim_owned(&consumer, LEASE, f.now()).unwrap()
        else {
            panic!("expected owned claim")
        };
        db.set_delivery(
            &lease,
            "old",
            json!({"pane":PANE,"state_change_seq":1,"submitted":false}),
            f.now(),
        )
        .unwrap();
        if submitted {
            db.set_delivery(
                &lease,
                "old",
                json!({"pane":PANE,"state_change_seq":1,"submitted":true}),
                f.now(),
            )
            .unwrap();
        }
        f.step(&postman, Step::NotOwner).await;
        elapsed(LEASE).await;
        f.tick(&postman).await;

        let delivery = f.item(&id).delivery.unwrap();
        assert_eq!(delivery == "old", submitted);
        assert_eq!(herdr.prompts(), usize::from(!submitted));
        let answer = f.push(&reply(&capsule, &delivery));
        f.step(&postman, Step::Replied(answer)).await;
        assert_eq!(f.item(&id).status, ItemStatus::Done);
    }
}

#[tokio::test(start_paused = true)]
async fn two_postmen_on_one_database_produce_one_prompt() {
    let f = Fixture::new(true).await;
    let (herdr, first) = fake();
    let second = Postman::new(herdr.clone());
    let separate = QueueService::default();
    separate.now();
    let id = f.push(&task("one"));
    let (a, b) = tokio::join!(f.tick(&first), separate.mail_tick(&f.server, &second));
    let steps = [a, b.unwrap()];
    assert!(
        steps
            .iter()
            .any(|s| matches!(s.as_slice(), [(_, Step::Delivered(_))]))
    );
    assert!(
        steps
            .iter()
            .any(|s| matches!(s.as_slice(), [(_, Step::NotOwner)]))
    );
    assert_eq!(f.item(&id).status, ItemStatus::Running);
    assert_eq!(herdr.prompts(), 1);
}

#[tokio::test(start_paused = true)]
async fn disabling_drains_inflight_and_removing_a_label_releases_its_lease() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let capsule = task("one");
    let id = f.push(&capsule);
    f.tick(&postman).await;

    let queued = f.push(&task("two"));
    f.setting(MAIL_ENABLED, Setting::Bool(false));
    let delivery = f.item(&id).delivery.unwrap();
    let answer = f.push(&reply(&capsule, &delivery));
    f.step(&postman, Step::Replied(answer)).await;
    assert_eq!(f.item(&id).status, ItemStatus::Done);
    assert_eq!(f.item(&queued).status, ItemStatus::Queued);
    assert_eq!(herdr.prompts(), 1);

    f.setting(MAIL_ENABLED, Setting::Bool(true));
    herdr.pane("working", 2);
    f.step(&postman, Step::Busy).await;
    assert!(
        f.db()
            .consumers()
            .unwrap()
            .iter()
            .any(|row| row.consumer.queue == MAILBOX && row.until > f.now())
    );
    f.labels(&[]);
    f.tick(&postman).await;
    let mut db = f.db();
    assert!(
        db.consumers()
            .unwrap()
            .iter()
            .all(|row| row.consumer.queue != MAILBOX || row.until <= f.now())
    );
    assert!(
        db.claim_consumer(MAILBOX, "next-owner", LEASE, f.now())
            .unwrap()
            .is_some()
    );
}

#[tokio::test(start_paused = true)]
async fn raw_capsule_control_characters_never_reach_a_pane() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    for capsule in [
        Capsule {
            summary: "one\ntwo".into(),
            ..task("newline")
        },
        Capsule {
            from: "lead\u{1b}".into(),
            ..task("control")
        },
    ] {
        let id = f.push(&capsule);
        f.tick(&postman).await;
        assert_eq!(f.item(&id).status, ItemStatus::Dead);
    }
    assert_eq!(herdr.prompts(), 0);
    herdr_queue::run_audit(&f.path(), None, None).unwrap();
}

#[tokio::test(start_paused = true)]
async fn non_task_finishes_when_the_turn_changes_and_is_idle_again() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let id = f.push(&Capsule {
        status: "wip".into(),
        ..task("notice")
    });
    f.tick(&postman).await;
    f.step(&postman, Step::Waiting).await;
    herdr.pane("working", 2);
    f.step(&postman, Step::Waiting).await;
    assert_eq!(f.item(&id).status, ItemStatus::Running);
    herdr.pane("idle", 3);
    f.step(&postman, Step::TurnEnded).await;
    assert_eq!(f.item(&id).status, ItemStatus::Done);
}

#[tokio::test(start_paused = true)]
async fn missing_sequence_neither_admits_nor_confirms_a_turn() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let id = f.push(&task("one"));
    let pane = lock(&herdr.panes).pop().unwrap();
    *lock(&herdr.panes) = vec![MailPane { seq: None, ..pane }];
    f.step(&postman, Step::Busy).await;
    assert_eq!(herdr.prompts(), 0);
    assert_eq!(f.item(&id).status, ItemStatus::Queued);

    herdr.pane("idle", 1);
    f.tick(&postman).await;
    let pane = lock(&herdr.panes).pop().unwrap();
    *lock(&herdr.panes) = vec![MailPane { seq: None, ..pane }];
    f.step(&postman, Step::Waiting).await;
    elapsed(CONFIRM).await;
    f.step(&postman, Step::Waiting).await;
    assert_eq!(f.item(&id).status, ItemStatus::Running);
    assert_eq!(herdr.enters(), 0);

    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let id = f.push(&Capsule {
        status: "wip".into(),
        ..task("notice")
    });
    f.tick(&postman).await;
    let pane = lock(&herdr.panes).pop().unwrap();
    *lock(&herdr.panes) = vec![MailPane { seq: None, ..pane }];
    f.step(&postman, Step::Waiting).await;
    assert_eq!(f.item(&id).status, ItemStatus::Running);
}

#[tokio::test(start_paused = true)]
async fn missing_or_ambiguous_panes_prevent_typing() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let id = f.push(&task("one"));
    let pane = lock(&herdr.panes).pop().unwrap();
    f.step(&postman, Step::NoPane).await;
    *lock(&herdr.panes) = vec![
        pane.clone(),
        MailPane {
            pane_id: "w1:p8".into(),
            ..pane
        },
    ];
    f.step(&postman, Step::NoPane).await;
    assert_eq!(herdr.prompts(), 0);
    assert_eq!(f.item(&id).status, ItemStatus::Queued);
}

#[tokio::test(start_paused = true)]
async fn closed_pane_then_replacement_gets_one_redelivery() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let capsule = task("one");
    let id = f.push(&capsule);
    f.tick(&postman).await;
    let first = f.item(&id).delivery.unwrap();

    herdr.pane("working", 2);
    f.step(&postman, Step::Waiting).await;
    // A relabelled pane is still alive, even though its mailbox label is gone.
    lock(&herdr.panes)[0].label.clear();
    f.step(&postman, Step::Waiting).await;
    assert_eq!(f.item(&id).status, ItemStatus::Running);
    lock(&herdr.panes).clear();
    assert!(matches!(
        f.tick(&postman).await.as_slice(),
        [(_, Step::Released(_))]
    ));
    let (released, events) = f.db().item(&id).unwrap().unwrap();
    assert_eq!(released.status, ItemStatus::Queued);
    assert_eq!(released.attempts, 1);
    assert_eq!(events[events.len() - 2].kind, "failed");
    assert_eq!(events.last().unwrap().kind, "requeued");
    assert!(
        f.db()
            .list(Some("mail-return"), None, 10)
            .unwrap()
            .is_empty()
    );

    herdr.pane("idle", 1);
    lock(&herdr.panes)[0].pane_id = "w1:p8".into();
    f.tick(&postman).await;
    let second = f.item(&id);
    assert_eq!(second.status, ItemStatus::Running);
    assert_eq!(second.attempts, 2);
    let delivery = second.delivery.unwrap();
    assert_ne!(delivery, first);
    assert!(lock(&herdr.calls).iter().any(|call| matches!(
        call, Call::Prompt(pane, text) if pane == "w1:p8" && text.contains(&delivery)
    )));

    f.step(&postman, Step::Waiting).await;
    let answer = f.push(&reply(&capsule, &delivery));
    f.step(&postman, Step::Replied(answer)).await;
    assert_eq!(f.item(&id).status, ItemStatus::Done);
    assert_eq!(herdr.prompts(), 2);
    assert_eq!(herdr.enters(), 0);
    herdr_queue::run_audit(&f.path(), None, None).unwrap();
}

#[tokio::test(start_paused = true)]
async fn closed_pane_preserves_a_committed_reply_and_respects_attempt_budget() {
    for answered in [false, true] {
        let f = Fixture::new(true).await;
        let (herdr, postman) = fake();
        let capsule = task("one");
        let payload = serde_json::to_value(&capsule).unwrap();
        let pushed = f
            .db()
            .push(
                Push {
                    queue: MAILBOX.into(),
                    payload: payload.as_object().unwrap().clone(),
                    dedupe_key: None,
                    coalesce_key: None,
                    requester: None,
                    max_attempts: 1,
                    max_depth: None,
                },
                f.now(),
            )
            .unwrap();
        let Pushed::New(item) = pushed else {
            unreachable!("empty mailbox")
        };
        let id = item.id;
        f.tick(&postman).await;
        lock(&herdr.panes).clear();
        if answered {
            let answer = f.push(&reply(&capsule, &f.item(&id).delivery.unwrap()));
            f.step(&postman, Step::Replied(answer)).await;
            assert_eq!(f.item(&id).status, ItemStatus::Done);
        } else {
            assert!(matches!(
                f.tick(&postman).await.as_slice(),
                [(_, Step::Dead(_))]
            ));
            assert_eq!(f.item(&id).status, ItemStatus::Dead);
        }
        assert_eq!(f.item(&id).attempts, 1);
        assert_eq!(herdr.prompts(), 1);
        herdr_queue::run_audit(&f.path(), None, None).unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn all_terminal_delivery_paths_pass_queue_audit() {
    let f = Fixture::new(true).await;
    let (herdr, postman) = fake();
    let capsule = task("answered");
    let done = f.push(&capsule);
    let notice = f.push(&Capsule {
        status: "wip".into(),
        ..task("notice")
    });
    let dead = f.push(&task("expired"));
    f.tick(&postman).await;
    f.push(&reply(&capsule, &f.item(&done).delivery.unwrap()));
    f.tick(&postman).await;
    f.tick(&postman).await;
    herdr.pane("idle", 2);
    f.step(&postman, Step::TurnEnded).await;
    f.tick(&postman).await;
    elapsed(WAIT).await;
    f.tick(&postman).await;
    assert_eq!(f.item(&done).status, ItemStatus::Done);
    assert_eq!(f.item(&notice).status, ItemStatus::Done);
    assert_eq!(f.item(&dead).status, ItemStatus::Dead);
    herdr_queue::run_audit(&f.path(), None, None).unwrap();
}

#[tokio::test(start_paused = true)]
async fn prompt_failure_refunds_the_claim_for_clean_redelivery() {
    let f = Fixture::new(true).await;
    let herdr = Arc::new(FakeMailHerdr {
        fail_prompt: true,
        ..Default::default()
    });
    herdr.pane("idle", 1);
    let postman = Postman::new(herdr.clone());
    let id = f.push(&task("one"));
    let steps = f.tick(&postman).await;
    assert!(matches!(steps.as_slice(), [(_, Step::Released(_))]));
    assert_eq!(herdr.prompts(), 1);
    let item = f.item(&id);
    assert_eq!(item.status, ItemStatus::Queued);
    assert_eq!(item.attempts, 0);
    herdr_queue::run_audit(&f.path(), None, None).unwrap();
}

#[tokio::test(start_paused = true)]
async fn unknown_label_mail_goes_dead_with_one_notice() {
    let f = Fixture::new(true).await;
    f.setting(MAIL_UNDELIVERABLE_AFTER_MS, Setting::Number(1_000));
    let (herdr, postman) = fake();
    lock(&herdr.panes).clear();
    let capsule = task("missing");
    let id = f.push(&capsule);
    f.step(&postman, Step::NoPane).await; // the absence starts here

    elapsed(1_000).await;
    f.step(&postman, Step::NoPane).await;
    assert_eq!(f.item(&id).status, ItemStatus::Queued);

    let conn = rusqlite::Connection::open(f.path()).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_notice BEFORE INSERT ON queue_items
         WHEN NEW.queue = 'mail-return'
         BEGIN SELECT RAISE(ABORT, 'notice insertion failed'); END;",
    )
    .unwrap();
    elapsed(1).await;
    f.tick(&postman).await;
    assert_eq!(f.item(&id).status, ItemStatus::Queued);
    assert_eq!(f.db().item(&id).unwrap().unwrap().1.len(), 1);
    conn.execute_batch("DROP TRIGGER reject_notice").unwrap();

    f.step(
        &postman,
        Step::Dead(format!("undeliverable: no pane with label {LABEL}")),
    )
    .await;
    f.step(&postman, Step::NoPane).await;
    let (item, events) = f.db().item(&id).unwrap().unwrap();
    assert_eq!(item.status, ItemStatus::Dead);
    assert_eq!(item.attempts, 0);
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].kind, "dead");
    assert_eq!(events[1].data.as_ref().unwrap()["undeliverable"], true);
    let notices = f.db().list(Some("mail-return"), None, 10).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].payload["status"], "fail");
    assert_eq!(notices[0].payload["re"], capsule.id);
    assert!(!notices[0].payload.contains_key("delivery"));
    assert!(f.db().list(Some("mail-lead"), None, 10).unwrap().is_empty());
    assert_eq!(herdr.prompts(), 0);
    herdr_queue::run_audit(&f.path(), None, None).unwrap();
}

#[tokio::test(start_paused = true)]
async fn duplicate_label_mail_is_never_undeliverable() {
    let f = Fixture::new(true).await;
    f.setting(MAIL_UNDELIVERABLE_AFTER_MS, Setting::Number(1_000));
    let (herdr, postman) = fake();
    let id = f.push(&task("ambiguous"));
    elapsed(1_001).await;

    let step = f
        .server
        .queues
        .mail_step(&f.server, &postman, LABEL, &[], true, false)
        .await
        .unwrap();
    assert_eq!(step, Step::Empty);
    assert_eq!(f.item(&id).status, ItemStatus::Queued);

    {
        let mut panes = lock(&herdr.panes);
        let duplicate = MailPane {
            pane_id: "w1:p8".into(),
            ..panes[0].clone()
        };
        panes.push(duplicate);
    }

    f.step(&postman, Step::NoPane).await;
    assert_eq!(f.item(&id).status, ItemStatus::Queued);
    assert!(
        f.db()
            .list(Some("mail-return"), None, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(herdr.prompts(), 0);
}

#[tokio::test(start_paused = true)]
async fn old_mail_survives_a_brief_pane_absence() {
    // An agent restart hides its pane for a moment; a backlog that waited
    // longer than the delay behind a busy agent must not all go dead.
    let f = Fixture::new(true).await;
    f.setting(MAIL_UNDELIVERABLE_AFTER_MS, Setting::Number(1_000));
    let (herdr, postman) = fake();
    herdr.pane("working", 1);
    let id = f.push(&task("backlog"));
    f.step(&postman, Step::Busy).await;
    elapsed(5_000).await;
    f.step(&postman, Step::Busy).await;

    let gone = std::mem::take(&mut *lock(&herdr.panes));
    f.step(&postman, Step::NoPane).await;
    elapsed(999).await;
    f.step(&postman, Step::NoPane).await;
    assert_eq!(f.item(&id).status, ItemStatus::Queued);

    *lock(&herdr.panes) = gone;
    f.step(&postman, Step::Busy).await;
    lock(&herdr.panes).clear();
    elapsed(999).await;
    f.step(&postman, Step::NoPane).await; // the clock restarted when it came back
    assert_eq!(f.item(&id).status, ItemStatus::Queued);
    assert!(
        f.db()
            .list(Some("mail-return"), None, 10)
            .unwrap()
            .is_empty()
    );
}
