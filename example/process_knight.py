# Copyright (C) 2026 Mutantcat Working Group
# SPDX-License-Identifier: GPL-3.0-only
# 由异猫工作群（mutantcat.org）发行 · GitHub: https://github.com/Mutantcat-Working-Group

from PIL import Image
import os

from pathlib import Path

def process_knight_to_32x32(input_path, output_path):
    """
    将骑士精灵图处理成32x32的角色帧
    原始：1536x1024，包含8个384x512的帧（2行4列）
    目标：每个帧缩小到32x32
    """
    # 打开原始图片
    img = Image.open(input_path)
    width, height = img.size

    print(f"原始图片尺寸: {width}x{height}")

    # 计算原始帧尺寸（2行4列）
    cols = 4
    rows = 2
    original_frame_width = width // cols  # 384
    original_frame_height = height // rows  # 512

    print(f"原始帧尺寸: {original_frame_width}x{original_frame_height}")

    # 目标帧尺寸
    target_frame_width = 32
    target_frame_height = 32

    # 创建新的精灵图
    new_frames = []

    # 遍历原始帧（2行4列）
    for row in range(rows):
        for col in range(cols):
            # 计算当前帧的位置
            left = col * original_frame_width
            top = row * original_frame_height
            right = left + original_frame_width
            bottom = top + original_frame_height

            # 裁剪出原始帧
            frame = img.crop((left, top, right, bottom))

            # 缩小到32x32（使用NEAREST保持像素风格）
            frame_32x32 = frame.resize((target_frame_width, target_frame_height), Image.NEAREST)

            new_frames.append(frame_32x32)
            print(f"处理帧 ({row}, {col}): 原始位置 ({left}, {top}, {right}, {bottom}) -> 缩小到 {target_frame_width}x{target_frame_height}")

    # 创建新的精灵图（2行4列，每个32x32）
    new_width = cols * target_frame_width  # 128
    new_height = rows * target_frame_height  # 64

    new_sprite_sheet = Image.new('RGBA', (new_width, new_height), (0, 0, 0, 0))

    # 将处理后的帧拼接到新精灵图上
    for i, frame in enumerate(new_frames):
        row = i // cols
        col = i % cols

        left = col * target_frame_width
        top = row * target_frame_height
        new_sprite_sheet.paste(frame, (left, top))

    # 保存结果
    new_sprite_sheet.save(output_path)
    print(f"\n✅ 处理完成！")
    print(f"新精灵图尺寸: {new_width}x{new_height}")
    print(f"每个帧尺寸: {target_frame_width}x{target_frame_height}")
    print(f"输出路径: {output_path}")

    return new_sprite_sheet

if __name__ == "__main__":
    # 样例跟着 example/ 走：按自己所在目录找图，换机器也跑得起来。
    refer_dir = Path(__file__).parent / "refer_img"
    input_file = str(refer_dir / "骑士.png")
    output_file = str(refer_dir / "骑士_32x32.png")

    process_knight_to_32x32(input_file, output_file)
