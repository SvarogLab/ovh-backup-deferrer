//! The hourly cycle over the configured services: what to write, and when.

use std::time::Duration;

use anyhow::{Result, bail};

use jiff::civil::Time;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use tracing::{error, info, warn};

use crate::config::Service;
use crate::ovh::{Client, RescheduleError};

/// Pauses between attempts at one service within a cycle, after a transient failure. The API's
/// internal errors are sporadic and pass within a minute or so.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(10), Duration::from_secs(60)];

/// Cycles in a row a service may end without a write before the daemon gives up. A write keeps
/// the backup at least 22 hours away, so six hours of outage still leave most of a day to notice.
const MAX_MISSED_CYCLES: u32 = 6;

/// Runs a cycle now and then shortly after every full UTC hour, until a cycle fails.
pub async fn run(client: &Client, services: &[Service], dry_run: bool) -> Result<()> {
    let mut missed = vec![0; services.len()];
    loop {
        run_cycle(
            client,
            services,
            Timestamp::now(),
            dry_run,
            &RETRY_DELAYS,
            &mut missed,
        )
        .await?;
        tokio::time::sleep(until_next_run(Timestamp::now())).await;
    }
}

/// Writes every service's target for `now`, one after another, retrying transient failures
/// after each of `retry_delays`. `missed` counts, per service, the cycles in a row that ended
/// without a write. A failed service does not stop the others, but fails the cycle if its
/// failure is permanent or it has now missed `MAX_MISSED_CYCLES`.
async fn run_cycle(
    client: &Client,
    services: &[Service],
    now: Timestamp,
    dry_run: bool,
    retry_delays: &[Duration],
    missed: &mut [u32],
) -> Result<()> {
    let mut failed = 0;
    for (service, missed) in services.iter().zip(missed) {
        let name = service.name.as_str();
        let target = target(now, service.offset);
        if dry_run {
            info!(service = name, %target, outcome = "dry-run");
            continue;
        }
        match reschedule(client, name, target, retry_delays).await {
            Ok(()) => {
                info!(service = name, %target, outcome = "written");
                *missed = 0;
            }
            Err(error) if error.is_transient() && *missed + 1 < MAX_MISSED_CYCLES => {
                *missed += 1;
                warn!(
                    service = name, %target, outcome = "missed", missed_cycles = *missed,
                    error = format!("{error:#}")
                );
            }
            Err(error) => {
                error!(service = name, %target, outcome = "failed", error = format!("{error:#}"));
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} services failed", services.len());
    }
    Ok(())
}

/// One service's write, repeated after each of `retry_delays` while it fails transiently.
async fn reschedule(
    client: &Client,
    name: &str,
    target: Time,
    retry_delays: &[Duration],
) -> Result<(), RescheduleError> {
    let mut delays = retry_delays.iter();
    loop {
        match client.reschedule(name, target).await {
            Err(error) if error.is_transient() => {
                let Some(&delay) = delays.next() else {
                    return Err(error);
                };
                warn!(
                    service = name, %target, outcome = "retrying", after = ?delay,
                    error = format!("{error:#}")
                );
                tokio::time::sleep(delay).await;
            }
            result => return result,
        }
    }
}

/// The schedule to write at `now`: the start of the previous UTC hour, plus the offset.
fn target(now: Timestamp, offset: u8) -> Time {
    let hour = now.to_zoned(TimeZone::UTC).time().hour();
    Time::midnight()
        .wrapping_add(SignedDuration::from_hours(i64::from(hour)))
        .wrapping_add(SignedDuration::from_mins(i64::from(offset) - 60))
}

/// How long to sleep from `now` until ten seconds past the start of the next UTC hour; waking
/// a little after the hour rather than on it leaves no doubt which hour `now` is in.
fn until_next_run(now: Timestamp) -> Duration {
    Duration::from_secs((3610 - now.as_second().rem_euclid(3600)).unsigned_abs())
}

#[cfg(test)]
mod tests {
    use jiff::civil::time;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::ovh::tests::client_of;

    fn services() -> [Service; 2] {
        [
            Service {
                name: "vps-a.example".into(),
                offset: 0,
            },
            Service {
                name: "vps-b.example".into(),
                offset: 20,
            },
        ]
    }

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn just_after_the_hour() -> Timestamp {
        at("2026-09-28T15:00:10Z")
    }

    const NO_DELAY: [Duration; 2] = [Duration::ZERO; 2];

    /// Answers every reschedule of `service` to `schedule` with `status`, at most `times` times
    /// if given, and expects `expected` requests.
    async fn answer(
        server: &MockServer,
        service: &str,
        schedule: &str,
        status: u16,
        times: Option<u64>,
        expected: u64,
    ) {
        let mock = Mock::given(method("POST"))
            .and(path(format!(
                "/1.0/vps/{service}/automatedBackup/reschedule"
            )))
            .and(body_json(serde_json::json!({ "schedule": schedule })))
            .respond_with(ResponseTemplate::new(status));
        let mock = match times {
            Some(times) => mock.up_to_n_times(times),
            None => mock,
        };
        mock.expect(expected).mount(server).await;
    }

    async fn cycle(server: &MockServer, missed: &mut [u32]) -> Result<()> {
        run_cycle(
            &client_of(server),
            &services(),
            just_after_the_hour(),
            false,
            &NO_DELAY,
            missed,
        )
        .await
    }

    #[tokio::test]
    async fn a_transient_failure_is_retried_and_a_write_clears_the_missed_count() {
        let server = MockServer::start().await;
        answer(&server, "vps-a.example", "14:00:00", 500, Some(1), 1).await;
        answer(&server, "vps-a.example", "14:00:00", 200, None, 1).await;
        answer(&server, "vps-b.example", "14:20:00", 200, None, 1).await;
        let mut missed = [3, 0];

        cycle(&server, &mut missed).await.unwrap();

        assert_eq!(missed, [0, 0]);
    }

    #[tokio::test]
    async fn a_service_failing_every_retry_is_missed_without_failing_the_cycle() {
        let server = MockServer::start().await;
        answer(&server, "vps-a.example", "14:00:00", 500, None, 3).await;
        answer(&server, "vps-b.example", "14:20:00", 200, None, 1).await;
        let mut missed = [0, 0];

        cycle(&server, &mut missed).await.unwrap();

        assert_eq!(missed, [1, 0]);
    }

    #[tokio::test]
    async fn too_many_missed_cycles_in_a_row_fail_the_cycle_after_every_service_is_written() {
        let server = MockServer::start().await;
        answer(&server, "vps-a.example", "14:00:00", 500, None, 3).await;
        answer(&server, "vps-b.example", "14:20:00", 200, None, 1).await;
        let mut missed = [MAX_MISSED_CYCLES - 1, 0];

        assert!(cycle(&server, &mut missed).await.is_err());
    }

    #[tokio::test]
    async fn a_permanent_failure_is_not_retried_and_fails_the_cycle_after_every_service_is_written()
    {
        let server = MockServer::start().await;
        answer(&server, "vps-a.example", "14:00:00", 403, None, 1).await;
        answer(&server, "vps-b.example", "14:20:00", 200, None, 1).await;
        let mut missed = [0, 0];

        assert!(cycle(&server, &mut missed).await.is_err());
    }

    #[tokio::test]
    async fn dry_run_writes_nothing() {
        let server = MockServer::start().await;

        run_cycle(
            &client_of(&server),
            &services(),
            just_after_the_hour(),
            true,
            &NO_DELAY,
            &mut [0, 0],
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert!(requests.is_empty());
    }

    #[test]
    fn target_is_the_previous_hour_plus_the_offset() {
        for (now, minutes, expected) in [
            ("2026-09-28T15:37:12.5Z", 20, time(14, 20, 0, 0)),
            ("2026-09-28T00:20:00Z", 40, time(23, 40, 0, 0)),
        ] {
            assert_eq!(target(at(now), minutes), expected, "{now} +{minutes}");
        }
    }

    #[test]
    fn next_run_is_ten_seconds_into_the_next_hour() {
        assert_eq!(
            until_next_run(at("2026-09-28T14:59:50Z")),
            Duration::from_secs(20)
        );
    }
}
