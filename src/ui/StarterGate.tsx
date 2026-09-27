import { Button } from "antd";
import { KeyRound, Server, Workflow } from "lucide-react";

import { useStore } from "../lib/store";
import { useT } from "../lib/t";

export default function StarterGate() {
  const t = useT();
  const openSettings = useStore((s) => s.openSettings);

  return (
    <div className="gate">
      <div className="gate-inner">
        <h1>AIPixel</h1>
        <p>
          {t("gate.pitch")}
        </p>
        <div className="gate-points">
          <div className="gate-point">
            <span className="k">1</span>
            <span>{t("gate.point1")}</span>
          </div>
          <div className="gate-point">
            <span className="k">2</span>
            <span>{t("gate.point2")}</span>
          </div>
          <div className="gate-point">
            <span className="k">3</span>
            <span>{t("gate.point3")}</span>
          </div>
        </div>
        <div>
          <Button type="primary" icon={<KeyRound size={14} />} onClick={openSettings}>
            {t("gate.add_first")}
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
