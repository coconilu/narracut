import {
  NARRACUT_CONTRACT_VERSION,
  NARRACUT_PROJECT_COMMAND_API_VERSION,
} from "@narracut/contracts";
import "./App.css";

function App() {
  return (
    <main
      className="container"
      data-contract-version={NARRACUT_CONTRACT_VERSION}
      data-project-command-version={NARRACUT_PROJECT_COMMAND_API_VERSION}
    >
      <h1>NarraCut（叙剪）</h1>
      <p>可观察、可编辑、可重跑的视频创作工作台。</p>
      <p>
        本地项目服务已就绪；项目首页与完整工作台将在后续 UI 里程碑接入。
      </p>
    </main>
  );
}

export default App;
