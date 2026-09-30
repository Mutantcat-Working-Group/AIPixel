# Copyright (C) 2026 Mutantcat Working Group
# SPDX-License-Identifier: GPL-3.0-only
# 由异猫工作群（mutantcat.org）发行 · GitHub: https://github.com/Mutantcat-Working-Group

from PIL import Image

from pathlib import Path

def process_attack_to_32x32(input_path, output_path):
    """
    将攻击角色图片处理成32x32
    原始：64x64单帧
    目标：32x32
    """
    # 打开原始图片
    img = Image.open(input_path)
    width, height = img.size

    print(f"原始图片尺寸: {width}x{height}")

    # 目标尺寸
    target_size = 32

    # 缩小到32x32（使用NEAREST保持像素风格）
    img_32x32 = img.resize((target_size, target_size), Image.NEAREST)

    # 保存结果
    img_32x32.save(output_path)
    print(f"\n✅ 处理完成！")
    print(f"新图片尺寸: {target_size}x{target_size}")
    print(f"输出路径: {output_path}")

    return img_32x32

if __name__ == "__main__":
    # 样例跟着 example/ 走：按自己所在目录找图，换机器也跑得起来。
    refer_dir = Path(__file__).parent / "refer_img"
    input_file = str(refer_dir / "攻击.png")
    output_file = str(refer_dir / "攻击_32x32.png")

    process_attack_to_32x32(input_file, output_file)
