/**
 * App.tsx — Root: QueryClient + Auth + Near + Router + Layout
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { HashRouter, Routes, Route, Navigate } from "react-router-dom";
import { AuthProvider } from "./hooks/useAuth";
import { NearProvider } from "./hooks/useNearWallet";
import { Layout } from "./components/Layout";
import FeedPage from "./pages/FeedPage";
import IdentityPage from "./pages/IdentityPage";
import GovernancePage from "./pages/GovernancePage";

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, staleTime: 30_000 } },
});

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <NearProvider>
          <HashRouter>
            <Layout>
              <Routes>
                <Route path="/" element={<FeedPage />} />
                <Route path="/identity" element={<IdentityPage />} />
                <Route path="/governance" element={<GovernancePage />} />
                <Route path="*" element={<Navigate to="/" replace />} />
              </Routes>
            </Layout>
          </HashRouter>
        </NearProvider>
      </AuthProvider>
    </QueryClientProvider>
  );
}
