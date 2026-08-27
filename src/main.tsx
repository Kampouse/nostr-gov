import { StrictMode, Component, type ReactNode, type ErrorInfo } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App'

class ErrorBoundary extends Component<{children: ReactNode}, {error: Error | null}> {
  state = { error: null as Error | null }
  static getDerivedStateFromError(error: Error) { return { error } }
  componentDidCatch(error: Error, info: ErrorInfo) { console.error('[ErrorBoundary]', error, info) }
  render() {
    if (this.state.error) {
      return (
        <div style={{padding:24,color:'#f43f5e',fontFamily:'monospace',fontSize:13,whiteSpace:'pre-wrap'}}>
          <h2 style={{color:'#e4e4e7',marginBottom:8}}>Runtime Error</h2>
          {this.state.error.message}
          {'\n\n'}
          {this.state.error.stack?.slice(0, 2000)}
        </div>
      )
    }
    return this.props.children
  }
}

createRoot(document.getElementById('root')!).render(
  <ErrorBoundary>
    <StrictMode>
      <App />
    </StrictMode>
  </ErrorBoundary>,
)