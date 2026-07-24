import assert from "node:assert/strict";
import test from "node:test";
import { collectAsyncDisposers, type Disposer } from "../src/asyncDisposers.ts";

test("disposes listeners that resolve after cleanup", async () => {
  let resolveProvider: ((dispose: Disposer) => void) | undefined;
  let resolveView: ((dispose: Disposer) => void) | undefined;
  let disposeCount = 0;
  const provider = new Promise<Disposer>((resolve) => {
    resolveProvider = resolve;
  });
  const view = new Promise<Disposer>((resolve) => {
    resolveView = resolve;
  });

  const cleanup = collectAsyncDisposers([provider, view]);
  cleanup();
  resolveProvider?.(() => {
    disposeCount += 1;
  });
  resolveView?.(() => {
    disposeCount += 1;
  });
  await Promise.resolve();
  await Promise.resolve();

  assert.equal(disposeCount, 2);
});
