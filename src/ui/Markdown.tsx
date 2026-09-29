import { memo, useMemo, type ReactNode } from "react";

import { parseMarkdown, type BlockNode, type InlineNode } from "../lib/markdown";

/** 行内节点 -> React。没有 dangerouslySetInnerHTML，模型输出永远进不了 DOM 解析器。 */
function renderInline(nodes: InlineNode[]): ReactNode[] {
  return nodes.map((node, index) => {
    switch (node.kind) {
      case "code":
        return (
          <code className="md-code" key={index}>
            {node.value}
          </code>
        );
      case "strong":
        return <strong key={index}>{renderInline(node.children)}</strong>;
      case "em":
        return <em key={index}>{renderInline(node.children)}</em>;
      case "del":
        return <del key={index}>{renderInline(node.children)}</del>;
      default:
        return <span key={index}>{node.value}</span>;
    }
  });
}

function renderBlock(block: BlockNode, index: number): ReactNode {
  switch (block.kind) {
    case "heading": {
      // 只到 h4：h5/h6 在 13px 的聊天区里和正文一样大，纯占噪音。
      const level = Math.min(block.level, 4);
      const Tag = `h${level}` as "h1" | "h2" | "h3" | "h4";
      return <Tag key={index}>{renderInline(block.children)}</Tag>;
    }
    case "code":
      return (
        <pre className="md-pre" key={index}>
          {block.lang ? <span className="md-lang">{block.lang}</span> : null}
          <code>{block.value}</code>
        </pre>
      );
    case "list":
      return block.ordered ? (
        <ol key={index}>
          {block.items.map((item, itemIndex) => (
            <li key={itemIndex}>{renderInline(item)}</li>
          ))}
        </ol>
      ) : (
        <ul key={index}>
          {block.items.map((item, itemIndex) => (
            <li key={itemIndex}>{renderInline(item)}</li>
          ))}
        </ul>
      );
    case "quote":
      return (
        <blockquote className="md-quote" key={index}>
          {renderInline(block.children)}
        </blockquote>
      );
    case "table":
      return (
        <div className="md-table-wrap" key={index}>
          <table className="md-table">
            <thead>
              <tr>
                {block.head.map((cell, cellIndex) => (
                  <th key={cellIndex}>{renderInline(cell)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, rowIndex) => (
                <tr key={rowIndex}>
                  {row.map((cell, cellIndex) => (
                    <td key={cellIndex}>{renderInline(cell)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "rule":
      return <hr className="md-rule" key={index} />;
    default:
      return <p key={index}>{renderInline(block.children)}</p>;
  }
}

/**
 * 把助手正文渲染成排版好的内容。流式输出时每个 token 都会重新解析一次，
 * 所以外面套 memo：只在文本真的变了时才重算。
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  const blocks = useMemo(() => parseMarkdown(text), [text]);
  if (text.trim() === "") return null;
  return <div className="md">{blocks.map(renderBlock)}</div>;
});

export default Markdown;
