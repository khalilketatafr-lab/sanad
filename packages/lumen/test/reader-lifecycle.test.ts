/**
 * Drives the Reader Shell lifecycle (src/reader/lifecycle.ts) with mock ports —
 * no browser, no worker, no Kernel — to prove the blueprint's state graph and,
 * above all, its security invariant: no state reachable before a page is drawn
 * exposes content. A grant primes and reads; a denial lands on the gate (never
 * the error state); a transient failure is retryable; network loss enters
 * offline-grace and a silent renewal returns to reading.
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import { createActor, type SnapshotFrom } from "xstate";

import { KernelError } from "../src/vault/kernel-client.ts";
import {
  classifyOpenError,
  readerMachine,
  type LeaseInfo,
  type ReaderPorts,
  type Viewport,
} from "../src/reader/lifecycle.ts";

const VIEWPORT: Viewport = { width: 800, height: 1200, dpr: 2 };
const LEASE: LeaseInfo = { leaseId: "lease-1", window: [0, 3], expiresIn: 300 };
const RENEWED: LeaseInfo = { leaseId: "lease-2", window: [0, 3], expiresIn: 300 };

type Snapshot = SnapshotFrom<typeof readerMachine>;

function stubPorts(overrides: Partial<ReaderPorts> = {}): ReaderPorts {
  return {
    authorize: async () => ({ kind: "granted", lease: LEASE }),
    prime: async () => ({ anchor: [0, 0, 0] }),
    land: async (r) => ({ anchor: r.target }),
    renew: async () => RENEWED,
    ...overrides,
  };
}

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void } {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function start(ports: ReaderPorts, editionId = "ed-1") {
  return createActor(readerMachine, { input: { ports, editionId, viewport: VIEWPORT } });
}

/** Resolves when a snapshot satisfies `predicate`; rejects on timeout. */
function waitFor(
  actor: ReturnType<typeof start>,
  predicate: (s: Snapshot) => boolean,
  timeoutMs = 2000,
): Promise<Snapshot> {
  return new Promise((resolve, reject) => {
    const settle = (s: Snapshot): boolean => {
      if (!predicate(s)) return false;
      clearTimeout(timer);
      sub.unsubscribe();
      resolve(s);
      return true;
    };
    const timer = setTimeout(() => {
      sub.unsubscribe();
      reject(new Error(`timeout at ${JSON.stringify(actor.getSnapshot().value)}`));
    }, timeoutMs);
    const sub = actor.subscribe(settle);
    settle(actor.getSnapshot());
  });
}

const inState = (value: string) => (s: Snapshot) => s.matches(value as never);

test("a granted open primes and reaches reading with the lease held", async () => {
  const actor = start(stubPorts());
  actor.start();

  const reading = await waitFor(actor, inState("reading"));
  assert.ok(reading.hasTag("content"), "reading exposes content");
  assert.deepEqual(reading.context.lease, LEASE);
  assert.equal(reading.context.denial, undefined);
  actor.stop();
});

test("a denied open lands on the gate — a designed screen, not an error", async () => {
  const actor = start(stubPorts({ authorize: async () => ({ kind: "denied", denial: { reason: "sign-in", status: 403 } }) }));
  const seen: string[] = [];
  actor.subscribe((s) => seen.push(JSON.stringify(s.value)));
  actor.start();

  const gate = await waitFor(actor, inState("gate"));
  assert.equal(gate.context.denial?.reason, "sign-in");
  assert.ok(gate.hasTag("blocked"));
  assert.ok(!gate.hasTag("content"), "the gate never shows content");
  assert.ok(!seen.includes('"failed"'), "a denial is not routed through the error state");
  actor.stop();
});

test("content is never exposed before the first page is drawn (P1 invariant)", async () => {
  const before = deferred<void>();
  // Hold priming open so we can observe every pre-reading snapshot.
  const actor = start(stubPorts({ prime: async () => (await before.promise, { anchor: [0, 0, 0] }) }));
  const snapshots: Snapshot[] = [];
  actor.subscribe((s) => snapshots.push(s));
  actor.start();

  await waitFor(actor, inState("priming"));
  // Nothing drawn yet: not one snapshot so far may carry the content tag.
  assert.ok(snapshots.length > 0);
  for (const s of snapshots) {
    assert.ok(!s.hasTag("content"), `content leaked in ${JSON.stringify(s.value)} before reading`);
  }
  const values = new Set(snapshots.map((s) => JSON.stringify(s.value)));
  assert.ok(values.has('"opening"') && values.has('"priming"'), "passed through opening then priming");

  before.resolve();
  const reading = await waitFor(actor, (s) => s.hasTag("content"));
  assert.ok(reading.matches("reading"), "the first content-bearing state is reading");
  actor.stop();
});

test("a transient open failure lands on a retryable error state", async () => {
  let attempt = 0;
  const actor = start(
    stubPorts({
      authorize: async () => {
        attempt += 1;
        if (attempt === 1) throw new KernelError(503, "unavailable", "kernel down");
        return { kind: "granted", lease: LEASE };
      },
    }),
  );
  actor.start();

  const failed = await waitFor(actor, inState("failed"));
  assert.ok(failed.hasTag("error"));
  assert.ok(!failed.hasTag("content"));

  actor.send({ type: "RETRY" });
  const reading = await waitFor(actor, inState("reading"));
  assert.equal(reading.context.error, undefined, "a successful retry clears the error");
  assert.equal(attempt, 2);
  actor.stop();
});

test("network loss enters offline-grace and a silent renewal returns to reading", async () => {
  const renew = deferred<LeaseInfo>();
  const actor = start(stubPorts({ renew: () => renew.promise }));
  actor.start();
  await waitFor(actor, inState("reading"));

  actor.send({ type: "NETWORK_LOST" });
  const offline = await waitFor(actor, inState("offline_grace"));
  // Still reading the already-leased window while renewal is in flight.
  assert.ok(offline.hasTag("content") && offline.hasTag("offline"));

  renew.resolve(RENEWED);
  const back = await waitFor(actor, inState("reading"));
  assert.deepEqual(back.context.lease, RENEWED, "the lease was renewed silently");
  actor.stop();
});

test("while offline a failed renewal waits, keeps content, and retries on reconnect", async () => {
  let attempt = 0;
  const actor = start(
    stubPorts({
      renew: async () => {
        attempt += 1;
        if (attempt === 1) throw new KernelError(0, "offline", "no network");
        return RENEWED;
      },
    }),
  );
  actor.start();
  await waitFor(actor, inState("reading"));

  actor.send({ type: "NETWORK_LOST" });
  const waiting = await waitFor(actor, (s) => s.matches({ offline_grace: "waiting" }));
  assert.ok(waiting.hasTag("content"), "the leased window stays readable while offline");

  actor.send({ type: "NETWORK_RESTORED" });
  const back = await waitFor(actor, inState("reading"));
  assert.deepEqual(back.context.lease, RENEWED);
  assert.equal(attempt, 2);
  actor.stop();
});

test("classifyOpenError treats 401/403 as sign-in and everything else as transient", () => {
  assert.deepEqual(classifyOpenError(new KernelError(403, "access_denied", "no")), { reason: "sign-in", status: 403 });
  assert.deepEqual(classifyOpenError(new KernelError(401, "invalid_token", "no")), { reason: "sign-in", status: 401 });
  assert.equal(classifyOpenError(new KernelError(503, "unavailable", "down")), undefined);
  assert.equal(classifyOpenError(new TypeError("Failed to fetch")), undefined);
});
