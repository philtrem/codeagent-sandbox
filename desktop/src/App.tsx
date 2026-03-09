import Layout from "./components/Layout";
import ToastContainer from "./components/Toast";
import { useClaudeConfigSync } from "./hooks/useClaudeConfigSync";

export default function App() {
  useClaudeConfigSync();

  return (
    <>
      <Layout />
      <ToastContainer />
    </>
  );
}
