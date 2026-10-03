/**
 * The Reader Shell lifecycle (blueprint 03 §1.2), as an XState v5 machine.
 *
 *   opening --> priming      lease granted
 *   opening --> gate         denied  (a designed "sign in to keep reading"
 *                                      screen, never an error dialog)
 *   priming --> reading      first page drawn
 *   reading --> jumping      jump(intent)
 *   jumping --> reading      landed
 *   reading --> offline_grace network lost (keeps reading the leased window)
 *   offline_grace --> reading lease renewed silently
 *
 * The invariant the blueprint states in prose is encoded in the tags: the
 * `content` tag is present only once a page has actually been drawn
 * (`reading`, `jumping`, `offline_grace`). No state reachable while
 * authorization is unresolved — `opening`, `priming`, `gate`, `failed` — carries
 * it, so a shell that shows content iff `snapshot.hasTag("content")` can never
 * leak a page before the open is granted and primed.
 *
 * P1 / thread topology (03 §1.1): this machine runs on the main thread, which
 * "never holds glyph data, keys, or decrypted bytes." So its context holds only
 * anchors (UI state) and lease *metadata* (window, expiry) — never the wrapped
 * key material and never the laid-out page. The real work lives behind
 * [`ReaderPorts`], which a shell implements over the Vault Worker: `authorize`
 * hands the wire lease to the worker (which unwraps the chunk keys into
 * non-extractable `CryptoKey`s and keeps them), `prime`/`land` decode and lay
 * out in the worker and resolve with just the landed anchor once the Render
 * Worker has drawn, and `renew` refreshes the lease silently. The machine is
 * deterministic given its ports, which is what makes it testable without a
 * browser, a worker or a Kernel.
 */
import { assign, fromPromise, setup } from "xstate";

import { KernelError } from "../vault/kernel-client.ts";

/** A position independent of font, size, viewport or device (03 §3.3). */
export type Anchor = readonly [chunk: number, block: number, cluster: number];

/** How a jump was initiated (03 §1.3); drives the landing animation. */
export type NavIntent = "toc" | "search" | "bookmark" | "annotation" | "progress-bar" | "resume";

/**
 * Why an open was denied. Phase 1a has no purchase store, so every denial is
 * `sign-in` ("sign in to keep reading"); `subscribe` is reserved for the
 * Phase 2 buy/subscribe screen.
 */
export type DeniedReason = "sign-in" | "subscribe" | "unknown";

export interface Viewport {
  readonly width: number;
  readonly height: number;
  readonly dpr: number;
}

/** Lease metadata the shell may hold on the main thread. No key material. */
export interface LeaseInfo {
  readonly leaseId: string;
  /** `[start, end)` chunk window the lease covers. */
  readonly window: readonly [number, number];
  /** Seconds until the lease expires. */
  readonly expiresIn: number;
}

export interface Denial {
  readonly reason: DeniedReason;
  /** The Kernel HTTP status that produced the denial, for diagnostics. */
  readonly status: number;
}

/** The outcome of an `authorize`: a grant is not an error, nor is a denial. */
export type OpenOutcome =
  | { readonly kind: "granted"; readonly lease: LeaseInfo }
  | { readonly kind: "denied"; readonly denial: Denial };

/** What `prime`/`land` resolve with: the anchor drawn, never any glyph data. */
export interface PrimeResult {
  readonly anchor: Anchor;
}

/**
 * The side effects the lifecycle drives, each a boundary to the Vault/Render
 * workers. A port rejects only on a *transient* failure (network, worker
 * crash); an expected, designed outcome — a denial — resolves normally as an
 * [`OpenOutcome`], so it lands on the gate rather than the error state.
 */
export interface ReaderPorts {
  authorize(req: { editionId: string; anchor: Anchor; viewport: Viewport }): Promise<OpenOutcome>;
  prime(req: { lease: LeaseInfo; anchor: Anchor; viewport: Viewport }): Promise<PrimeResult>;
  land(req: { lease: LeaseInfo; target: Anchor; intent: NavIntent; viewport: Viewport }): Promise<PrimeResult>;
  renew(req: { editionId: string; lease: LeaseInfo }): Promise<LeaseInfo>;
}

export interface ReaderInput {
  readonly ports: ReaderPorts;
  readonly editionId: string;
  /** Where to open; defaults to the start of the edition. */
  readonly anchor?: Anchor;
  readonly viewport: Viewport;
}

export interface ReaderContext {
  readonly ports: ReaderPorts;
  readonly editionId: string;
  viewport: Viewport;
  anchor: Anchor;
  lease: LeaseInfo | undefined;
  denial: Denial | undefined;
  /** The last transient failure, surfaced by the `failed` state or a toast. */
  error: string | undefined;
}

export type ReaderEvent =
  | { type: "JUMP"; target: Anchor; intent: NavIntent }
  | { type: "SET_ANCHOR"; anchor: Anchor }
  | { type: "NETWORK_LOST" }
  | { type: "NETWORK_RESTORED" }
  | { type: "AUTHENTICATED" }
  | { type: "RETRY" };

const START: Anchor = [0, 0, 0];

/** Silent re-attempt cadence while offline, ms. */
const RENEW_RETRY_MS = 5_000;

/**
 * Maps an open failure to a [`Denial`] when it is really a refusal, or
 * `undefined` when it is transient and should be retried. A Kernel `403`
 * (`access_denied`) or `401` (no/expired session) is the reader asking the
 * person to sign in; everything else — a network error, a `5xx` — is transient.
 * Lives here, tested, so every port that calls the Kernel classifies denials
 * the same way.
 */
export function classifyOpenError(err: unknown): Denial | undefined {
  if (err instanceof KernelError && (err.status === 403 || err.status === 401)) {
    return { reason: "sign-in", status: err.status };
  }
  return undefined;
}

/** Reads `event.output` off an `xstate.done.*` event, typed by the caller. */
function output<T>(event: object): T | undefined {
  return "output" in event ? (event as { output: T }).output : undefined;
}

export const readerMachine = setup({
  types: {
    context: {} as ReaderContext,
    input: {} as ReaderInput,
    events: {} as ReaderEvent,
  },
  actors: {
    authorize: fromPromise(
      ({ input }: { input: { ports: ReaderPorts; editionId: string; anchor: Anchor; viewport: Viewport } }) =>
        input.ports.authorize({ editionId: input.editionId, anchor: input.anchor, viewport: input.viewport }),
    ),
    prime: fromPromise(({ input }: { input: { ports: ReaderPorts; lease: LeaseInfo; anchor: Anchor; viewport: Viewport } }) =>
      input.ports.prime({ lease: input.lease, anchor: input.anchor, viewport: input.viewport }),
    ),
    land: fromPromise(
      ({
        input,
      }: {
        input: { ports: ReaderPorts; lease: LeaseInfo; target: Anchor; intent: NavIntent; viewport: Viewport };
      }) => input.ports.land({ lease: input.lease, target: input.target, intent: input.intent, viewport: input.viewport }),
    ),
    renew: fromPromise(({ input }: { input: { ports: ReaderPorts; editionId: string; lease: LeaseInfo } }) =>
      input.ports.renew({ editionId: input.editionId, lease: input.lease }),
    ),
  },
  guards: {
    granted: ({ event }) => output<OpenOutcome>(event)?.kind === "granted",
  },
  actions: {
    assignGrantedLease: assign(({ event }) => {
      const out = output<OpenOutcome>(event);
      return out?.kind === "granted" ? { lease: out.lease, denial: undefined, error: undefined } : {};
    }),
    assignDenial: assign(({ event }) => {
      const out = output<OpenOutcome>(event);
      return out?.kind === "denied" ? { denial: out.denial } : {};
    }),
    assignRenewedLease: assign(({ event }) => {
      const lease = output<LeaseInfo>(event);
      return lease === undefined ? {} : { lease, error: undefined };
    }),
    assignLanded: assign(({ event }) => {
      const landed = output<PrimeResult>(event);
      return landed === undefined ? {} : { anchor: landed.anchor, error: undefined };
    }),
    assignAnchor: assign(({ event }) => (event.type === "SET_ANCHOR" ? { anchor: event.anchor } : {})),
    assignError: assign(({ event }) => {
      const err = "error" in event ? (event as { error: unknown }).error : undefined;
      return { error: err instanceof Error ? err.message : String(err ?? "reader error") };
    }),
  },
}).createMachine({
  id: "reader",
  context: ({ input }) => ({
    ports: input.ports,
    editionId: input.editionId,
    viewport: input.viewport,
    anchor: input.anchor ?? START,
    lease: undefined,
    denial: undefined,
    error: undefined,
  }),
  initial: "opening",
  states: {
    opening: {
      tags: ["busy", "authorizing"],
      invoke: {
        src: "authorize",
        input: ({ context }) => ({
          ports: context.ports,
          editionId: context.editionId,
          anchor: context.anchor,
          viewport: context.viewport,
        }),
        onDone: [
          { target: "priming", guard: "granted", actions: "assignGrantedLease" },
          { target: "gate", actions: "assignDenial" },
        ],
        onError: { target: "failed", actions: "assignError" },
      },
    },

    priming: {
      tags: ["busy", "priming"],
      invoke: {
        src: "prime",
        input: ({ context }) => {
          if (context.lease === undefined) throw new Error("priming without a lease");
          return { ports: context.ports, lease: context.lease, anchor: context.anchor, viewport: context.viewport };
        },
        onDone: { target: "reading", actions: "assignLanded" },
        onError: { target: "failed", actions: "assignError" },
      },
    },

    reading: {
      tags: ["content", "reading"],
      on: {
        JUMP: "jumping",
        NETWORK_LOST: "offline_grace",
        SET_ANCHOR: { actions: "assignAnchor" },
      },
    },

    jumping: {
      // The previous page is still on screen while the next one lands, so the
      // content tag stays — authorization was resolved long ago.
      tags: ["content", "navigating"],
      invoke: {
        src: "land",
        input: ({ context, event }) => {
          if (context.lease === undefined) throw new Error("jump without a lease");
          if (event.type !== "JUMP") throw new Error("jumping entered without a JUMP event");
          return {
            ports: context.ports,
            lease: context.lease,
            target: event.target,
            intent: event.intent,
            viewport: context.viewport,
          };
        },
        // A failed jump is non-fatal: stay on the page the reader is already on.
        onDone: { target: "reading", actions: "assignLanded" },
        onError: { target: "reading", actions: "assignError" },
      },
      on: { NETWORK_LOST: "offline_grace" },
    },

    offline_grace: {
      // Still reading the already-leased window; only renewal is blocked.
      tags: ["content", "offline"],
      initial: "renewing",
      states: {
        renewing: {
          invoke: {
            src: "renew",
            input: ({ context }) => {
              if (context.lease === undefined) throw new Error("renew without a lease");
              return { ports: context.ports, editionId: context.editionId, lease: context.lease };
            },
            onDone: { target: "#reader.reading", actions: "assignRenewedLease" },
            onError: { target: "waiting", actions: "assignError" },
          },
        },
        waiting: {
          after: { [RENEW_RETRY_MS]: "renewing" },
          on: { NETWORK_RESTORED: "renewing" },
        },
      },
    },

    gate: {
      // A designed screen (sign in / upsell), not a dead end. When the shell
      // completes sign-in it sends AUTHENTICATED and the open is re-attempted.
      tags: ["blocked", "gate"],
      on: { AUTHENTICATED: "opening", RETRY: "opening" },
    },

    failed: {
      // A transient failure: a spinner would be a dead end, so the shell shows
      // a retry affordance that sends RETRY.
      tags: ["error"],
      on: { RETRY: "opening" },
    },
  },
});

export type ReaderMachine = typeof readerMachine;
