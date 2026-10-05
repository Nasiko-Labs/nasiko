import React, { Component, ErrorInfo, ReactNode } from 'react';
import { AlertOctagon, RotateCcw, Home, Sparkles } from 'lucide-react';

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
  errorInfo: ErrorInfo | null;
}

export class ErrorBoundary extends Component<Props, State> {
  public state: State = {
    hasError: false,
    error: null,
    errorInfo: null,
  };

  public static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error, errorInfo: null };
  }

  public componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error('[ErrorBoundary caught exception]:', error, errorInfo);
    this.setState({ errorInfo });
  }

  private handleReset = () => {
    localStorage.removeItem('ai_coach_session_id');
    window.location.reload();
  };

  private handleClearAllAndHome = () => {
    localStorage.clear();
    window.location.href = '/';
  };

  public render() {
    if (this.state.hasError) {
      if (this.props.fallback) {
        return this.props.fallback;
      }

      return (
        <div className="min-h-screen bg-slate-950 text-slate-100 flex items-center justify-center p-4">
          <div className="max-w-lg w-full bg-slate-900 border border-slate-800 rounded-2xl p-6 sm:p-8 shadow-2xl space-y-6 text-center">
            <div className="w-14 h-14 rounded-2xl bg-rose-950/60 border border-rose-800/80 text-rose-400 flex items-center justify-center mx-auto shadow-lg shadow-rose-950/40">
              <AlertOctagon className="w-7 h-7" />
            </div>

            <div className="space-y-2">
              <div className="inline-flex items-center space-x-1.5 px-3 py-1 rounded-full bg-slate-800 text-slate-300 text-xs font-medium border border-slate-700">
                <Sparkles className="w-3.5 h-3.5 text-indigo-400" />
                <span>AI Interview Coach Self-Healing Guard</span>
              </div>
              <h2 className="text-xl font-bold text-white">Something interrupted the session</h2>
              <p className="text-xs text-slate-400 leading-relaxed">
                The application encountered an unexpected state and safely caught the exception to prevent system failure.
              </p>
            </div>

            {this.state.error && (
              <div className="bg-slate-950 border border-slate-800 rounded-xl p-3.5 text-left text-xs font-mono text-rose-300 overflow-x-auto max-h-36">
                <p className="font-bold text-rose-400 mb-1">{this.state.error.name}: {this.state.error.message}</p>
              </div>
            )}

            <div className="flex flex-col sm:flex-row items-center justify-center gap-3 pt-2">
              <button
                onClick={this.handleReset}
                className="w-full sm:w-auto px-4 py-2.5 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-semibold flex items-center justify-center space-x-2 transition-colors shadow-md shadow-indigo-500/20"
              >
                <RotateCcw className="w-4 h-4" />
                <span>Reload Session</span>
              </button>
              <button
                onClick={this.handleClearAllAndHome}
                className="w-full sm:w-auto px-4 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-300 text-xs font-semibold flex items-center justify-center space-x-2 border border-slate-700 transition-colors"
              >
                <Home className="w-4 h-4" />
                <span>Reset to Start</span>
              </button>
            </div>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
