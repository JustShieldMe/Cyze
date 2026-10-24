import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";
import { useCeremonies } from "../stores/ceremonies";

// Each test installs its own fake backend (fakeBackend), replacing the last
// one. The mocks aren't cleared in between: components unmounted here still
// unsubscribe from events a moment later, through the mock. The ceremony
// store lives at module level, so each test starts it empty.
afterEach(() => {
  cleanup();
  useCeremonies.setState({
    ceremonies: {},
    activeDkgId: null,
    activeSigningId: null,
    activeSendByGroup: {},
    txSigners: {},
    joinedSessions: {},
  });
});
