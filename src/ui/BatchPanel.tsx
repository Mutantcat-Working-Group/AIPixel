// 批量工作台：一个文件夹进、一个文件夹出，纯本机批处理，和 agent 会话互补那一半。
// 会话是「一次一两张、一个模型盯着」，这里是「一个 token 都不花」的确定性重复劳动，
// 所以面板里没有任何模型控件：两个文件夹、一遍扫描、一个开始，就是全部。
// 过程走 batch-event，逐文件折进行表；跑完的回执带成败计数，失败原因跟在自己那一行上。

import { Alert, Button, InputNumber, Progress, Segmented, Switch } from "antd";
import { FolderOpen, Layers, Play, ScanSearch, X } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";

import { Field, QuantizeFields } from "./QuantizeFields";
import { batchPercent, canRunBatch, scanMatchesKind } from "../lib/batch";
import { useStore } from "../lib/store";
import { useT } from "../lib/t";
import type { BatchKind, ExportFormat } from "../lib/types";

function baseName(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

/** 文件夹行：选好了显示目录名（悬停看全路径），空着就是一个选择按钮。 */
function DirField({
  label,
  value,
  onPick,
  onClear,
}: {
  label: string;
  value: string;
  onPick: () => void;
  onClear: () => void;
}) {
  const t = useT();
  if (value !== "") {
    return (
      <Field label={label}>
        <div className="dock-path">
          <span className="grow" title={value}>
            {baseName(value)}
          </span>
          <Button size="small" type="text" icon={<X size={12} />} onClick={onClear} />
        </div>
      </Field>
    );
  }
  return (
    <Field label={label}>
      <Button size="small" block icon={<FolderOpen size={12} />} onClick={onPick}>
        {t("batch.pick")}
      </Button>
    </Field>
  );
}

export default function BatchPanel() {
  const t = useT();
  const recipe = useStore((s) => s.recipe);
  const scan = useStore((s) => s.scan);
  const scanBusy = useStore((s) => s.scanBusy);
  const run = useStore((s) => s.run);
  const setBatchKind = useStore((s) => s.setBatchKind);
  const patchBatchRecipe = useStore((s) => s.patchBatchRecipe);
  const pickBatchInput = useStore((s) => s.pickBatchInput);
  const pickBatchOutput = useStore((s) => s.pickBatchOutput);
  const scanBatchInput = useStore((s) => s.scanBatchInput);
  const runBatch = useStore((s) => s.runBatch);
  const resetBatch = useStore((s) => s.resetBatch);

  // store 会在 kind / 目录变化时作废旧扫描，这里再确认一次：清单和 recipe 必须是同一码事。
  const scanned = scan !== null && scanMatchesKind(scan, recipe.kind);
  const canRun = scanned && canRunBatch(scan, run);

  async function pickDir(assign: (dir: string) => void) {
    const picked = await open({ directory: true });
    if (typeof picked === "string") assign(picked);
  }

  return (
    <section className="panel dock">
      <div className="panel-head">
        <Layers size={13} />
        <strong>{t("batch.title")}</strong>
      </div>

      <div className="panel-body dock-body">
        <div className="dock-panel">
          <p className="dock-note">{t("batch.blurb")}</p>

          <Field label={t("batch.kind")}>
            <Segmented
              size="small"
              block
              value={recipe.kind}
              options={[
                {
                  label: t("batch.kind.quantize"),
                  value: "quantize",
                  title: t("batch.kind.quantize.hint"),
                },
                {
                  label: t("batch.kind.export"),
                  value: "export",
                  title: t("batch.kind.export.hint"),
                },
              ]}
              onChange={(next) => setBatchKind(next as BatchKind)}
            />
          </Field>

          <DirField
            label={t("batch.input")}
            value={recipe.input_dir}
            onPick={() => void pickDir(pickBatchInput)}
            onClear={() => pickBatchInput("")}
          />
          <DirField
            label={t("batch.output")}
            value={recipe.output_dir}
            onPick={() => void pickDir(pickBatchOutput)}
            onClear={() => pickBatchOutput("")}
          />

          <Button
            size="small"
            block
            icon={<ScanSearch size={13} />}
            loading={scanBusy}
            disabled={recipe.input_dir === ""}
            onClick={() => void scanBatchInput()}
          >
            {scanned ? t("batch.rescan") : t("batch.scan")}
          </Button>

          {scanned && scan ? (
            <p className="dock-note">
              {scan.count > 0
                ? t("batch.scan_ready", { count: scan.count })
                : t("batch.scan_empty")}
            </p>
          ) : (
            <p className="dock-note">{t("batch.idle")}</p>
          )}

          {scan?.truncated ? (
            <Alert
              type="warning"
              showIcon
              className="dock-alert"
              message={t("batch.scan_truncated", { count: scan.count })}
            />
          ) : null}

          {recipe.kind === "quantize" ? (
            <>
              <div className="dock-flag">
                <Switch
                  size="small"
                  checked={recipe.match_source_size}
                  onChange={(next) => patchBatchRecipe({ match_source_size: next })}
                />
                <span>{t("batch.match_source")}</span>
              </div>
              {recipe.match_source_size ? null : (
                <Field label={t("batch.target_size")}>
                  <div className="batch-dims">
                    <InputNumber
                      size="small"
                      style={{ flex: 1, minWidth: 0 }}
                      min={1}
                      max={1024}
                      value={recipe.target_w}
                      onChange={(next) => patchBatchRecipe({ target_w: next ?? 64 })}
                    />
                    <span className="batch-dims-x">x</span>
                    <InputNumber
                      size="small"
                      style={{ flex: 1, minWidth: 0 }}
                      min={1}
                      max={1024}
                      value={recipe.target_h}
                      onChange={(next) => patchBatchRecipe({ target_h: next ?? 64 })}
                    />
                  </div>
                </Field>
              )}
              <QuantizeFields
                value={recipe.options}
                onChange={(options) => patchBatchRecipe({ options })}
              />
            </>
          ) : (
            <Field label={t("batch.export_format")}>
              <Segmented
                size="small"
                block
                value={recipe.export_format}
                options={[
                  { label: "PNG", value: "png" },
                  { label: "GIF", value: "gif" },
                ]}
                onChange={(next) => patchBatchRecipe({ export_format: next as ExportFormat })}
              />
            </Field>
          )}

          <Button
            size="small"
            block
            type="primary"
            icon={<Play size={12} />}
            loading={run.running}
            disabled={!canRun}
            onClick={() => void runBatch()}
          >
            {t("batch.run")}
          </Button>
        </div>

        {run.running ? (
          <div className="dock-panel">
            <Progress percent={batchPercent(run)} size="small" status="active" />
            <p className="dock-probe">
              {t("batch.running", { done: run.done, total: run.total })}
            </p>
          </div>
        ) : null}

        {run.rows.length > 0 ? (
          <div className="batch-rows">
            {run.rows.map((row, index) => (
              <div key={`${row.file}-${index}`} className={`batch-row ${row.state}`}>
                <span className="batch-row-state">{t(`batch.state.${row.state}`)}</span>
                <span className="batch-row-name" title={row.file}>
                  {row.file}
                </span>
                <span className="batch-row-note" title={row.note}>
                  {row.note}
                </span>
              </div>
            ))}
          </div>
        ) : null}

        {run.summary ? (
          <div className="dock-result">
            <div className="dock-result-summary">
              {t(run.summary.failed > 0 || run.summary.skipped > 0 ? "batch.done" : "batch.done_clean", {
                ok: run.summary.ok,
                skipped: run.summary.skipped,
                failed: run.summary.failed,
              })}
            </div>
            <p className="dock-note" title={run.summary.outputDir}>
              {t("batch.output_to", { dir: run.summary.outputDir })}
            </p>
            <div className="dock-actions">
              <Button size="small" onClick={resetBatch}>
                {t("batch.clear")}
              </Button>
            </div>
          </div>
        ) : null}

        {run.error ? (
          <Alert type="error" showIcon className="dock-alert" message={run.error} />
        ) : null}
      </div>
    </section>
  );
}
