import {
  NARRACUT_CONTRACT_VERSION,
  NARRACUT_PROJECT_COMMAND_API_VERSION,
} from "@narracut/contracts";
import { useAppController } from "./features/app/use-app-controller";
import { ProjectHome } from "./features/projects/project-home";
import { WorkbenchShell } from "./features/workbench/workbench-shell";
import "./App.css";

function App() {
  const controller = useAppController();

  return (
    <div
      className="narracut-app"
      data-contract-version={NARRACUT_CONTRACT_VERSION}
      data-gateway-mode={controller.gatewayMode}
      data-project-command-version={NARRACUT_PROJECT_COMMAND_API_VERSION}
    >
      {controller.workspace ? (
        <WorkbenchShell
          bundle={controller.workspace}
          busyLabel={controller.busyLabel}
          error={controller.error}
          key={controller.workspace.project.projectId}
          onBack={controller.closeWorkspace}
          onCancelJob={controller.cancelJob}
          onClearError={controller.clearError}
          onRecover={controller.recoverWorkspace}
          onRefresh={controller.refreshWorkspace}
        />
      ) : (
        <ProjectHome
          busyLabel={controller.busyLabel}
          drawerMode={controller.drawerMode}
          error={controller.error}
          gatewayMode={controller.gatewayMode}
          onClearError={controller.clearError}
          onCreate={controller.createProject}
          onDrawerModeChange={controller.setDrawerMode}
          onOpenPath={controller.openProjectPath}
          onOpenRecent={controller.openRecentProject}
          projects={controller.recentProjects}
        />
      )}
    </div>
  );
}

export default App;
