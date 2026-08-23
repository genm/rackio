// Replace a pairing bundle's advertised direct addresses.
//
// Why this exists: `rackio pairing create` fills `direct_addresses` from the
// machine's own interfaces (crates/rackio-iroh/src/pairing.rs via
// EndpointConfig). A machine behind NAT therefore advertises only its LAN
// address, and in direct-only mode nothing discovers the router's public
// address. The operator of a port-forwarded machine knows that address; the
// product currently gives them no supported way to put it in a bundle.
//
// The lab performs that substitution explicitly so the port-forwarded scenario
// tests the transport rather than the missing UX, and the report records that
// it happened. This is a lab affordance standing in for a product gap, not a
// supported workflow — see scripts/nat-lab/README.md.
//
// Usage:
//   node rewrite-bundle-addresses.mjs <bundle> <ip:port> [ip:port...]
//     stdout: the re-encoded bundle
//   node rewrite-bundle-addresses.mjs --read <bundle>
//     stdout: the bundle's current direct addresses as a JSON array
//
// The one-time secret is carried through untouched and never printed, so the
// report can record what was advertised without recording pairing material.

const argv = process.argv.slice(2);
const readOnly = argv[0] === "--read";
const [bundle, ...addresses] = readOnly ? argv.slice(1) : argv;

if (!bundle || (!readOnly && addresses.length === 0)) {
  process.stderr.write("usage: rewrite-bundle-addresses.mjs [--read] <bundle> [ip:port...]\n");
  process.exit(2);
}

const prefix = "rackio-pair:";
if (!bundle.startsWith(prefix)) {
  process.stderr.write("not a rackio pairing bundle\n");
  process.exit(2);
}

const decoded = JSON.parse(Buffer.from(bundle.slice(prefix.length), "base64url").toString("utf8"));

if (readOnly) {
  process.stdout.write(JSON.stringify(decoded.direct_addresses ?? []));
} else {
  decoded.direct_addresses = addresses;
  process.stdout.write(prefix + Buffer.from(JSON.stringify(decoded)).toString("base64url"));
}
