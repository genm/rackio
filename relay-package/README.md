# Self-hosted relay package

This directory packages the upstream `iroh-relay` binary at exact version
`1.0.3`. It is not a fork and stores no peer database or metrics history.

## Production setup

1. Copy `config.example.toml` to `config.toml`.
2. Replace the example endpoint allowlist with the exact IDs printed by
   `tray-monitor status`.
3. Mount a valid TLS certificate and private key under `certs/`.
4. Open TCP 80/443 and UDP 7824. Keep metrics port 9090 bound to localhost or a
   private management network.
5. Run `docker compose up --build -d`.
6. Configure each agent with `tray-monitor relay set
   https://relay.example.test` and restart it.

The reserved hostname above is documentation only. Use a hostname on the
certificate you control.

## Health and logs

```sh
docker compose ps
curl --fail http://127.0.0.1:9090/metrics
docker compose logs --since=10m relay
```

Relay logs and metrics reveal traffic metadata even though application payloads
remain end-to-end encrypted. Define an access-log retention policy appropriate
for your environment.

## Failure drill

Stop the relay and verify that existing direct peers remain live while
relay-only peers become offline:

```sh
docker compose stop relay
docker compose start relay
```

Do not claim a successful direct path from reachability alone; the desktop badge
must follow iroh's selected path.
