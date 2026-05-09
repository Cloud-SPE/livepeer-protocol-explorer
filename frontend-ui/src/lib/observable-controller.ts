import type { ReactiveController, ReactiveControllerHost } from 'lit';
import type { Observable, Subscription } from 'rxjs';

/**
 * Bridges an RxJS Observable to a Lit ReactiveControllerHost. Stores the latest
 * emission in `value` and triggers `host.requestUpdate()` on each change.
 */
export class ObservableController<T> implements ReactiveController {
  value: T | undefined;
  private sub: Subscription | null = null;

  constructor(
    private readonly host: ReactiveControllerHost,
    private readonly source$: Observable<T>,
    initial?: T,
  ) {
    if (initial !== undefined) this.value = initial;
    host.addController(this);
  }

  hostConnected(): void {
    this.sub = this.source$.subscribe((v) => {
      this.value = v;
      this.host.requestUpdate();
    });
  }

  hostDisconnected(): void {
    this.sub?.unsubscribe();
    this.sub = null;
  }
}
