import { Button } from "antd";
import { KeyRound, Server, Workflow } from "lucide-react";

import { useStore } from "../lib/store";

export default function StarterGate() {
  const openSettings = useStore((s) => s.openSettings);

  return (
    <div className="gate">
      <div className="gate-inner">
        <h1>AIPixel</h1>
        <p>
          Bring your own model. Nothing to sign up for, no server in the middle: the agent loop runs
          on your machine and talks straight to the provider you configure.
        </p>
        <div className="gate-points">
          <div className="gate-point">
            <span className="k">1</span>
            <span>Add a model with your own provider endpoint and API key.</span>
          </div>
          <div className="gate-point">
            <span className="k">2</span>
            <span>The key stays on this device and is never sent back to the webview.</span>
          </div>
          <div className="gate-point">
            <span className="k">3</span>
            <span>Then ask for a sprite; the canvas becomes the only source of truth.</span>
          </div>
        </div>
        <div>
          <Button type="primary" icon={<KeyRound size={14} />} onClick={openSettings}>
            Add your first model
          </Button>
        </div>
        <div className="gate-icons">
          <Server size={13} />
          <Workflow size={13} />
        </div>
      </div>
    </div>
  );
}
