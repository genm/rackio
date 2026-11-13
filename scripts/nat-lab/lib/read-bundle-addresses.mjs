// Print a pairing bundle's advertised direct addresses as a JSON array.
//
// Scenarios assert on what `rackio pairing create` actually advertises — the
// machine's own interface addresses plus anything the operator configured with
// `rackio advertise-address` — so a report records the addresses a viewer was
// handed rather than the lab's expectation of them.
//
// Read-only by design. An earlier version could also rewrite the addresses,
// which the port-forwarded scenario needed while the product had no supported
// way to advertise a forwarded address (#152). It has one now, so the lab
// imports the bundle exactly as it was produced, and the rewrite path is gone
// rather than left available to quietly paper over a future gap.
//
// Usage:
//   node read-bundle-addresses.mjs <bundle>
//
// The one-time secret is decoded but never printed, so a report can record what
// was advertised without recording pairing material.

const [bundle] = process.argv.slice(2);

if (!bundle) {
  process.stderr.write("usage: read-bundle-addresses.mjs <bundle>\n");
  process.exit(2);
}

const prefix = "rackio-pair:";
if (!bundle.startsWith(prefix)) {
  process.stderr.write("not a rackio pairing bundle\n");
  process.exit(2);
}

const decoded = JSON.parse(Buffer.from(bundle.slice(prefix.length), "base64url").toString("utf8"));
process.stdout.write(JSON.stringify(decoded.direct_addresses ?? []));
