# Self-hosted relay package

This directory packages the upstream `iroh-relay` binary at exact version
`1.0.3`. It is not a fork and stores no peer database or metrics history.

## Production setup

1. Copy `config.example.toml` to `config.toml`.
2. Replace the example endpoint allowlist with the exact IDs printed by
   `rackio status`.
3. Mount a valid TLS certificate and private key under `certs/`. "Valid" means
   valid to the agents that will use it — see "Which certificate the relay must
   present" below.
4. Open TCP 80/443 and UDP 7824. Keep metrics port 9090 bound to localhost or a
   private management network.
5. Run `docker compose up --build -d`.
6. Configure each agent with `rackio relay set
   https://relay.example.test` and restart it.

The reserved hostname above is documentation only. Use a hostname on the
certificate you control.

## Which certificate the relay must present

The relay's certificate is verified by each agent's TLS trust anchor, and there
are exactly two of them.

By default an agent trusts iroh's compiled-in WebPKI root set. The relay must
then present a certificate issued by a publicly trusted authority for the
hostname in the relay URL — a Let's Encrypt certificate for a name in public
DNS, for instance. This is the right setup for a relay that is reachable from
the internet under a name you own.

A relay on an internal network usually cannot have that. No public authority
will issue for an internal-only name or a private IP address, and organisations
running such networks generally already operate an internal CA. For that case
the operator pins the CA on each agent:

```sh
sudo rackio relay set https://relay.internal.example.test \
  --ca-certificate /etc/rackio/relay-ca.pem
sudo systemctl restart rackio.service
```

End to end, the internal-CA path is:

1. **Issue the relay's certificate from your internal CA.** The Subject
   Alternative Name must contain the exact host in the relay URL the agents are
   configured with. A certificate issued for a different name, or an IP
   certificate used with a hostname URL, is rejected by name verification no
   matter which CA signed it.
2. **Mount that certificate and its key under `certs/`**, exactly as for a
   public certificate. The relay itself is unchanged; nothing here is
   Rackio-specific and no relay configuration differs.
3. **Distribute the CA certificate to each monitored machine.** This is the
   *issuing authority's* certificate, PEM encoded, not the relay's own leaf
   certificate and not a private key. If your CA has an intermediate, include
   the chain the relay does not send; several `CERTIFICATE` blocks in one file
   are all used, which is also how you run two anchors during a CA rotation.
4. **Point each agent at it** with `rackio relay set <URL> --ca-certificate
   <PATH>`, then restart the agent. The path is read at every start, so
   rotating the CA is replacing the file's contents.

Two properties of the pin are worth being explicit about:

- It **replaces** the public root set for relay connections rather than adding
  to it. A pinned relay is not also accepted on a publicly issued certificate.
- The file is a trust anchor, not a secret. Its contents are public; its
  integrity is not. Whoever can write it chooses the authority that agent
  trusts for the relay. Store it root-owned and not writable by unprivileged
  users. See [`../docs/threat-model.md`](../docs/threat-model.md).

A CA file that is missing, unreadable, or not a usable certificate authority is
refused when it is configured, and the previous relay configuration is left
untouched. The agent never falls back to the public root set for a relay whose
pinned CA it could not load: it refuses to start and says which file failed.

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
