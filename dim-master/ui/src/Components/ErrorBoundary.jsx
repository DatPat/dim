import { Component } from "react";

/**
 * Catches render crashes so a component bug degrades into a visible,
 * recoverable error instead of unmounting the entire app (which users
 * experience as the page silently freezing/going blank).
 */
class ErrorBoundary extends Component {
  constructor(props) {
    super(props);
    this.state = { error: null, attempt: 0 };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  componentDidCatch(error, info) {
    console.error("[ErrorBoundary]", error, info?.componentStack);
  }

  retry = () => {
    this.setState((s) => ({ error: null, attempt: s.attempt + 1 }));
  };

  render() {
    if (this.state.error) {
      return (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            minHeight: "60vh",
            gap: 12,
            padding: 24,
            textAlign: "center",
          }}
        >
          <h2>Something went wrong</h2>
          <p style={{ opacity: 0.7, maxWidth: 480, fontSize: 14 }}>
            {String(this.state.error?.message || this.state.error)}
          </p>
          <button
            onClick={this.retry}
            style={{
              padding: "8px 20px",
              borderRadius: 6,
              border: "1px solid #3498db",
              background: "rgba(52, 152, 219, 0.25)",
              color: "inherit",
              cursor: "pointer",
              fontSize: 14,
            }}
          >
            Try again
          </button>
        </div>
      );
    }

    // Key remounts the subtree on retry so crashed local state resets.
    return <div key={this.state.attempt} style={{ display: "contents" }}>{this.props.children}</div>;
  }
}

export default ErrorBoundary;
