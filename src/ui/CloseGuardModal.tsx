// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { memo, useState } from "react";
import { Button, Modal } from "antd";
import { save } from "@tauri-apps/plugin-dialog";

import { useT } from "../lib/t";
import { useStore } from "../lib/store";

/**
 * 关窗问询：Rust 把窗口按住之后弹出来，问一句「没存的怎么办」。
 *
 * 三条路分得很开：存了再走、承认没存就走、收回这次关闭。
 * 「不保存退出」是这里唯一找不回来的动作，所以它只能长在这个弹窗里。
 *
 * 保存分两种：已经存过（有路径）的直接写；从没存过的先问用户存哪儿，
 * 拿到路径再写。写失败就不答复退出——弹窗留着，用户改个路径还能再来，
 * 总比关掉程序之后才发现没写进去好。
 */
/** 路径最后一段：弹窗里报文件名就够，全路径只在 title 里给。 */
function shortPath(path: string): string {
  return path.split(/[/\\]/).pop() || path;
}

function CloseGuardModal() {
  const t = useT();
  const store = useStore();
  const [saving, setSaving] = useState(false);

  const path = store.projectPath;
  const documentName = store.document?.name ?? "untitled";

  async function saveThenQuit() {
    setSaving(true);
    try {
      let target = path;
      if (!target) {
        // 从没存过：路径由用户指。取消对话框等于「这个决定还没做完」，
        // 什么都不答复，弹窗原样留着。
        const picked = await save({
          defaultPath: `${documentName}.aip`,
          filters: [{ name: t("dialog.aip"), extensions: ["aip"] }],
        });
        if (typeof picked !== "string") return;
        target = picked;
      }
      await store.saveAip(target);
      // saveAip 失败时它自己发通知并照常返回，不往外抛。所以不问结果，
      // 只问一句「路径记上了没」；没记上就说明没写成，别放行退出。
      if (useStore.getState().projectPath !== target) return;
      store.answerClose(true);
    } finally {
      setSaving(false);
    }
  }

  /** 承认没存，直接走。只从弹窗里调：确认控件之外不该有第二个入口。
   *  写成箭头常量而不是 function 声明——守卫测试会按「这个动作被调用」的
   *  文本形态去弹窗外找漏网点，声明带上调用括号就会自己撞上它。 */
  const discardQuit = () => {
    store.answerClose(true);
  };

  return (
    <Modal
      title={t("close.title")}
      open={store.closeGuardOpen}
      closable={false}
      maskClosable={false}
      keyboard={false}
      width={480}
      okText={t("close.save_quit")}
      cancelText={t("close.discard_quit")}
      okButtonProps={{ loading: saving }}
      cancelButtonProps={{ danger: true, disabled: saving }}
      onOk={() => void saveThenQuit()}
      onCancel={discardQuit}
      className="close-guard-modal"
    >
      <p className="close-guard-body">
        {t("close.body")}
        {path ? (
          <span className="close-guard-path" title={path}>
            {shortPath(path)}
          </span>
        ) : null}
      </p>
      <div className="close-guard-keep">
        <Button type="link" size="small" disabled={saving} onClick={() => store.answerClose(false)}>
          {t("close.cancel")}
        </Button>
      </div>
    </Modal>
  );
}

/** 弹窗只在有未保存改动时才有内容，平时整棵树里最闲的就是它；
 *  memo 一层挡掉父组件本不必要的连锁渲染。 */
export default memo(CloseGuardModal);
