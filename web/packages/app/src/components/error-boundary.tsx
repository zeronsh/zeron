import { Component, type ErrorInfo, type ReactNode } from "react";

/**
 * The root error boundary. React rethrows an uncaught render/commit error
 * after unmounting the root — a white screen with no recovery — so the one
 * mount point every render error passes through degrades to a reload
 * affordance instead. A reload is the only affordance: the app's state is
 * session-scoped and the engine keeps running, so it is always safe.
 */
interface RootErrorBoundaryProps {
  readonly children: ReactNode;
}

interface RootErrorBoundaryState {
  readonly error: Error | null;
}

export class RootErrorBoundary extends Component<RootErrorBoundaryProps, RootErrorBoundaryState> {
  state: RootErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): RootErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("Uncaught render error", error, info.componentStack);
  }

  render(): ReactNode {
    if (this.state.error !== null) {
      return (
        <div className="app-error" role="alert">
          <h1 className="app-error-title">Zeron hit an error</h1>
          <p className="app-error-detail">{this.state.error.message}</p>
          <button type="button" className="btn btn-solid" onClick={() => window.location.reload()}>
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
