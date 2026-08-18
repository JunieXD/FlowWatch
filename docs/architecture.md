# Architecture

FlowWatch separates authoritative accounting from best-effort attribution. This distinction is a product invariant, not just an implementation detail.

```text
macOS native counters ---------> physical minute ledger -----> status/interfaces/spikes
                                     (authoritative)

macOS nettop snapshots -------->
                                app five-minute ledger ------> apps/coverage
Clash/Mihomo controller ------->     (attributed)

                                  SQLite daily rollups
```

## Workspace Boundaries

- `flowwatch-core`: platform-neutral identities, observations, delta trackers, source confidence, and backend traits.
- `flowwatch-store`: platform-neutral SQLite schema, aggregation, retention, permissions, and queries.
- `flowwatch-clash`: optional cross-platform Mihomo controller provider.
- `flowwatch-macos`: macOS interface counters, `nettop`, `libproc`, and application bundle identity.
- `apps/flowwatch`: CLI orchestration, collector lifecycle, configuration, and LaunchAgent installation.

Platform APIs must remain outside `flowwatch-core` and `flowwatch-store`. A future Windows implementation should be introduced as a sibling backend crate rather than adding conditional Windows behavior throughout shared accounting code.

## Accounting Invariants

1. Physical totals come only from hardware `enN` interfaces reported by `networksetup`. Loopback, `utun`, bridge, and other virtual interfaces are excluded from the physical ledger.
2. Interface counters are native 64-bit `IFMIB_IFDATA` values. Absolute baselines are persisted so a collector restart does not create a gap. Counter rollback is treated as an interface reset.
3. Every poll runs one short-lived cumulative `nettop -L 1` snapshot. Direct flow IDs include PID, interface, protocol, and socket endpoints so identical multicast sockets on different interfaces cannot cross-contaminate their counters. The first observation of every flow, and the first observation after a counter reset, is baseline-only. Only later monotonic increments are attributed. Only rows on hardware `enN` interfaces enter direct attribution; `lo0` rows only populate an in-memory `(protocol, source address, source port) -> application` map with a 15-second lifetime.
4. Direct app samples exclude known Clash/Mihomo carrier executables. Otherwise the carrier would be counted once by `nettop` and again by Clash attribution.
5. Clash resolves an actor from controller process metadata first, then from the short-lived loopback socket map. A known connection ID keeps its resolved identity across transient lookup failures. An unresolved active connection is held for six seconds so delayed socket metadata can arrive; if it closes or the grace period expires, its bytes remain visible as unknown.
6. Clash TUN and internal flows are excluded from app attribution. LAN clients receive explicit `[LAN]` identities. Unknown applications remain queryable but do not count toward coverage.
7. All writes use saturating arithmetic and transactional SQLite upserts. Raw flow targets and socket endpoints are not persisted.
8. Every completed five-minute direct bucket is compared with the physical ledger. A bucket that exceeds physical bytes by more than both 1 MiB and 10% produces a quality warning. Data is never silently clipped or scaled.
9. Coverage may exceed 100% if a backend or operating-system source behaves unexpectedly. The CLI reports the raw ratio instead of clipping it, making accounting regressions visible.
10. Application identity paths are canonicalized before resolution. The macOS backend recognizes the outermost `.app` or `.app.bundle` and prefers its bundle identifier. Pathless process-name aliases are accepted only while they identify one unique application; ambiguity falls back to a separate `process:` identity. Query-time consolidation applies the same conservative rule to retained history and recognizes the bundle identifier embedded in macOS code-sign-clone paths.
11. Clash `unattributed` is the saturating difference between controller total bytes and known-application bytes. New samples additionally persist observed actor bytes, allowing the CLI to separate actor attribution coverage from non-actor/internal/unobserved traffic. Actor totals are still sampled flow deltas, so a flow that closes between polls remains unobserved. Legacy proxy rows use nullable actor columns and are never backfilled with guessed classifications. Unknown actor rows remain explicit and are never redistributed.

## Sampling And Storage

The default collector takes one `nettop` snapshot every three seconds and flushes once per minute. Snapshot output is drained concurrently and a five-second watchdog kills a stuck child. The one-shot form avoids the high continuous NetworkStatistics cost observed with an infinite `nettop -L 0` subscription. App usage defaults to five-minute buckets and can opt into one-minute buckets; physical and proxy totals always use one-minute buckets. The two application detail tables can coexist across setting changes and roll into the same daily ledger. WAL mode and a bounded journal keep normal write amplification low.

Direct socket baselines and tombstones are held for 24 hours and capped at 100,000 entries. A tombstone contains its flow ID (including PID, interface, and socket endpoints), counters, state, and timestamps, but no resolved application identity. The Clash counter tracker retains closed connection IDs for 15 minutes and is capped at 50,000 entries; the connection-resolution cache has the same cap. The most recently observed entries win when a cap is reached. These structures exist only in memory and are rebuilt after restart; none of the endpoints are persisted.

After 30 days, fine-grained rows are rolled into local-calendar daily buckets. Daily rows are retained for 365 days by default. Both values are configurable at install time.

Named query windows and explicit `--from`/`--to` windows share one range model. The explicit start is inclusive and the end is exclusive, but aggregate buckets cannot be divided: app results include intersecting one- or five-minute detail buckets, and physical/proxy results include intersecting one-minute buckets. Rolled-up history has daily resolution. Application queries clamp their lower bound to `attribution_started_at`; a range entirely before that boundary returns no application rows.

The `gaps` query joins physical and known-application ledgers by time bucket and ranks the positive physical remainder. It uses minute buckets only after the most recent switch into one-minute application detail; ranges that include legacy five-minute data use five-minute buckets. This prevents a five-minute aggregate from being presented as five invented minute values.

Secrets are intentionally not abstracted behind a platform credential store in `0.1`: the Clash secret is JSON inside the SQLite settings table. Directory and file modes provide local-user isolation, and every display path redacts the value.

`history_started_at` records the beginning of the physical ledger. `attribution_started_at` separately marks the beginning of the current attribution algorithm. Status keeps the complete daily physical total visible but calculates application coverage, Clash coverage, and quality checks only over the latter window. This permits attribution-engine migrations without mixing incompatible app ledgers.

## Standard And Enhanced Modes

Standard mode is available now and requires no privileged helper. Its process attribution is sampled and therefore incomplete by design.

The planned enhanced macOS mode will be a separately signed Network Extension provider behind the same observation model. It must:

- use an explicit user authorization and installation flow;
- expose per-app byte deltas without changing storage/query contracts;
- assign a distinct `enhanced` source so results remain auditable;
- avoid merging with standard samples until duplicate-flow rules are proven;
- communicate through a narrow, versioned IPC contract;
- fall back to standard mode when entitlement or activation is unavailable.

This work is deferred until full Xcode, signing identities, and the required Apple entitlement process are available. Endpoint Security alone is not treated as a network byte-accounting API.

## Windows Extension Point

A future Windows backend can combine system interface counters with ETW or another documented per-process network source. It should implement the shared backend contract and emit the same `AppIdentity`, absolute counters, and flow observations. Administrator/driver-assisted collection, if needed, should be an enhanced provider rather than a requirement for the portable core.

The CLI installer and service manager are platform shells: LaunchAgent code stays macOS-specific, while a Windows service/task implementation can reuse the core, store, retention, queries, JSON output, and Clash provider.
