import { describe, it, expect } from 'vitest';
import { BehaviorSubject } from 'rxjs';
import type { ReactiveController, ReactiveControllerHost } from 'lit';
import { ObservableController } from '../lib/observable-controller.js';

interface FakeHost extends ReactiveControllerHost {
  controllers: ReactiveController[];
  updateCount: number;
  fireConnect(): void;
  fireDisconnect(): void;
}

function makeHost(): FakeHost {
  const controllers: ReactiveController[] = [];
  let updateCount = 0;
  return {
    controllers,
    get updateCount() {
      return updateCount;
    },
    addController(c) {
      controllers.push(c);
    },
    removeController() {
      /* no-op */
    },
    requestUpdate() {
      updateCount += 1;
    },
    updateComplete: Promise.resolve(true),
    fireConnect() {
      for (const c of controllers) c.hostConnected?.();
    },
    fireDisconnect() {
      for (const c of controllers) c.hostDisconnected?.();
    },
  };
}

describe('ObservableController', () => {
  it('subscribes on hostConnected and stores latest value', () => {
    const subject = new BehaviorSubject<{ id: string } | null>(null);
    const host = makeHost();
    const ctrl = new ObservableController(host, subject.asObservable());
    host.fireConnect();
    expect(ctrl.value).toBeNull();
    expect(host.updateCount).toBe(1);
    subject.next({ id: 'x' });
    expect(ctrl.value).toEqual({ id: 'x' });
    expect(host.updateCount).toBe(2);
  });

  it('unsubscribes on hostDisconnected', () => {
    const subject = new BehaviorSubject('a');
    const host = makeHost();
    const ctrl = new ObservableController(host, subject.asObservable());
    host.fireConnect();
    host.fireDisconnect();
    subject.next('b');
    expect(ctrl.value).toBe('a');
    expect(host.updateCount).toBe(1);
  });

  it('honors initial value', () => {
    const subject = new BehaviorSubject('emitted');
    const host = makeHost();
    const ctrl = new ObservableController(host, subject.asObservable(), 'initial');
    expect(ctrl.value).toBe('initial');
    host.fireConnect();
    expect(ctrl.value).toBe('emitted');
  });
});
