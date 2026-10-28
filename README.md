# ovh-autobackup-deferrer

Keeps the OVHcloud VPS *automated backup* schedule permanently in the recent past, so the daily
backup — which stops the guest for up to a minute — never reaches its trigger time.

At startup and then ten seconds after every full UTC hour, it sets every configured VPS's backup
schedule to the start of the previous hour plus that VPS's own offset, whatever the schedule was.
The schedule therefore sits one to two hours minus the offset in the past, and the provider, which
does not run a schedule that has already elapsed today, keeps postponing it to tomorrow.

Two provider behaviours this rests on are observed, not documented by OVHcloud: the API's
`schedule` is UTC (the control panel labels it so), and a schedule set to a time already past today
does not run until the next day (confirmed on real VPS). If either changes, the daemon triggers
backups instead of deferring them.

Deferring the backup also means the provider keeps no restore point for these VPS. Have another
backup of whatever they hold in place before deploying.

## Behaviour in short

- Per VPS and cycle: one `POST /vps/{service}/automatedBackup/reschedule`, nothing read first.
- A failing VPS is logged and the cycle still writes the others, then the daemon exits non-zero:
  a revoked key or a mistyped service shows up as a restarting pod rather than a log line. After a
  write the schedule sits at least 22 hours from firing, which leaves ample time to notice.
- Offsets must differ between VPS: a backup pauses its guest, and two etcd members paused together
  lose quorum. Startup refuses a shared offset; keep them well apart, e.g. 0, 20 and 40.
- Output is JSON logs on stdout, one line per VPS per cycle. There is no listener and no probe.

## Configuration

Environment variables only.

| Variable | Default | Meaning |
| -------- | ------- | ------- |
| `OVH_ENDPOINT` | `ovh-ca` | API region: `ovh-eu`, `ovh-ca` or `ovh-us` |
| `OVH_APPLICATION_KEY` | — | required |
| `OVH_APPLICATION_SECRET` | — | required |
| `OVH_CONSUMER_KEY` | — | required |
| `DEFERRER_SERVICES` | — | required; `service:offset` pairs, offset in minutes 0–59, e.g. `vps-aaaa.vps.ovh.net:0,vps-bbbb.vps.ovh.net:20` |
| `DEFERRER_DRY_RUN` | `false` | debugging: `true` logs the targets without writing them |
| `RUST_LOG` | `info` | log filter |

## Credentials

1. Create an application for your region, e.g. `https://ca.api.ovh.com/createApp/`. It yields the
   application key and secret.
2. Request a consumer key limited to the one call the daemon makes:

   ```sh
   curl -s -X POST https://ca.api.ovh.com/1.0/auth/credential \
     -H "X-Ovh-Application: $OVH_APPLICATION_KEY" \
     -H 'Content-Type: application/json' \
     -d '{"accessRules":[{"method":"POST","path":"/vps/*/automatedBackup/reschedule"}]}'
   ```

   Open the returned `validationUrl`, log in, choose an unlimited validity and confirm; the returned
   `consumerKey` is then valid. A key that expires turns every cycle into a failure, and the backup
   fires about a day later.
3. VPS service names, if you do not have them at hand, are listed in the control panel or by
   `GET /vps`; the daemon's key cannot read them.

## Running locally

```sh
export OVH_APPLICATION_KEY=... OVH_APPLICATION_SECRET=... OVH_CONSUMER_KEY=...
export DEFERRER_SERVICES=vps-aaaa.vps.ovh.net:0,vps-bbbb.vps.ovh.net:20
DEFERRER_DRY_RUN=true cargo run
```

Without `DEFERRER_DRY_RUN=true` this writes the schedules.

## Deploying

A `v<version>` tag publishes, both as `<version>`:

- the image `ghcr.io/svaroglab/ovh-backup-deferrer`;
- the Helm chart `oci://ghcr.io/svaroglab/ovh-backup-deferrer/ovh-autobackup-deferrer`, which
  runs that image as a single-replica Deployment. [helm/values.yaml](helm/values.yaml) documents
  its settings.

1. Note the current backup time of every VPS from the control panel, somewhere private; rollback
   writes these back.
2. Create the secret with the API keys in the namespace you deploy to, by hand or through the
   cluster's secret store:

   ```sh
   kubectl -n <namespace> create secret generic ovh-api --from-env-file=<file>
   ```

   where `<file>` holds `OVH_APPLICATION_KEY`, `OVH_APPLICATION_SECRET` and `OVH_CONSUMER_KEY`, one
   `NAME=value` per line.

   A running pod does not see a changed secret: after rotating the keys, run
   `kubectl -n <namespace> rollout restart deployment/ovh-autobackup-deferrer`.
3. Write a values file, kept out of this repository, listing a single VPS at first:

   ```yaml
   credentialsSecret: ovh-api
   services:
     - name: vps-aaaa.vps.ovh.net
       offset: 0
   ```

4. Install:

   ```sh
   helm upgrade --install ovh-autobackup-deferrer      oci://ghcr.io/svaroglab/ovh-backup-deferrer/ovh-autobackup-deferrer      --version <version> -n <namespace> -f <values>
   ```

   For Argo CD, the same chart is the source with `repoURL: ghcr.io/svaroglab/ovh-backup-deferrer`
   and `chart: ovh-autobackup-deferrer`. The pod needs outbound HTTPS to the API and DNS, nothing
   else.
5. Check the logs: one `written` line for the VPS, and the new time in the control panel a few
   minutes later.
6. Watch it for 72 hours: no new restore point in the control panel and no guest pause at its old
   backup time. Then add the rest to `services` and upgrade again.

Rollback: `helm uninstall ovh-autobackup-deferrer -n <namespace>`, then set the noted backup times
back in the control panel, keeping them at least 20 minutes apart.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```
