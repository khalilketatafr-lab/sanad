// Phase 1 entitlement policy. The Kernel supplies counters from an atomic
// entitlement/session store; clients never provide authorization context.

// Anonymous devices may sample the first three chapters only.
permit (
  principal == Device::"anonymous",
  action == Action::"open",
  resource
)
when {
  context.entitlement == "sample" &&
  context.chapter < 3 &&
  context.active_sessions < 2
};

// Authenticated readers may open free editions in full.
permit (
  principal is User,
  action == Action::"open",
  resource
)
when {
  context.entitlement == "free" &&
  context.active_sessions < 2 &&
  context.active_devices <= 6
};

// A forbid always overrides permits, including future publisher policies.
forbid (
  principal,
  action == Action::"open",
  resource
)
when {
  context.active_devices > 6 ||
  context.active_sessions >= 2
};
