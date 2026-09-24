#!/usr/bin/env python3
"""
图片缩放转换器
将图片转换到指定大小，使用高质量重采样算法保持辨识度

由异猫工作群（mutantcat.org）发行 · GitHub: https://github.com/Mutantcat-Working-Group
"""

import os
import argparse
from pathlib import Path
from PIL import Image, ImageOps


def resize_with_aspect_ratio(img, target_size, maintain_aspect=True, background_color=(255, 255, 255, 255)):
    """
    缩放图片到目标大小，可选择是否保持宽高比
    
    Args:
        img: PIL Image 对象
        target_size: 目标尺寸 (width, height)
        maintain_aspect: 是否保持宽高比
        background_color: 背景颜色（保持宽高比时填充）
    
    Returns:
        缩放后的 PIL Image 对象
    """
    target_width, target_height = target_size
    original_width, original_height = img.size
    
    if maintain_aspect:
        # 保持宽高比，使用 letterbox 方式
        # 计算缩放比例
        ratio = min(target_width / original_width, target_height / original_height)
        new_width = int(original_width * ratio)
        new_height = int(original_height * ratio)
        
        # 使用 Lanczos 重采样算法（高质量）
        img_resized = img.resize((new_width, new_height), Image.LANCZOS)
        
        # 创建目标大小的画布
        if img.mode == 'RGBA':
            canvas = Image.new('RGBA', (target_width, target_height), background_color)
        else:
            canvas = Image.new('RGB', (target_width, target_height), background_color[:3])
        
        # 将缩放后的图片居中放置
        paste_x = (target_width - new_width) // 2
        paste_y = (target_height - new_height) // 2
        canvas.paste(img_resized, (paste_x, paste_y), img_resized if img.mode == 'RGBA' else None)
        
        return canvas
    else:
        # 直接缩放到目标大小
        return img.resize(target_size, Image.LANCZOS)


def smart_crop(img, target_size):
    """
    智能裁剪：优先保留图像中心区域的重要内容
    
    Args:
        img: PIL Image 对象
        target_size: 目标尺寸 (width, height)
    
    Returns:
        裁剪后的 PIL Image 对象
    """
    target_width, target_height = target_size
    original_width, original_height = img.size
    
    # 计算裁剪区域（中心裁剪）
    if original_width / original_height > target_width / target_height:
        # 宽度更宽，裁剪左右
        new_width = int(original_height * target_width / target_height)
        left = (original_width - new_width) // 2
        crop_box = (left, 0, left + new_width, original_height)
    else:
        # 高度更高，裁剪上下
        new_height = int(original_width * target_height / target_width)
        top = (original_height - new_height) // 2
        crop_box = (0, top, original_width, top + new_height)
    
    img_cropped = img.crop(crop_box)
    img_resized = img_cropped.resize(target_size, Image.LANCZOS)
    
    return img_resized


def quantize_colors(img, max_colors=256):
    """
    颜色量化：将图像颜色数减少到指定数量以内
    
    Args:
        img: PIL Image 对象
        max_colors: 最大颜色数
    
    Returns:
        量化后的 PIL Image 对象
    """
    # 转换为 P 模式进行颜色量化
    # 使用 adaptive 方法以获得更好的结果
    img_quantized = img.convert('P', palette=Image.ADAPTIVE, colors=max_colors)
    # 转换回 RGBA
    img_quantized = img_quantized.convert('RGBA')
    return img_quantized


def process_image(input_path, output_dir, target_size, mode='resize', background_color=(255, 255, 255, 255), quantize=False, max_colors=256):
    """
    处理单个图片文件
    
    Args:
        input_path: 输入图片路径
        output_dir: 输出目录
        target_size: 目标尺寸 (width, height)
        mode: 缩放模式 ('resize'=直接缩放, 'letterbox'=保持宽高比填充, 'crop'=智能裁剪)
        background_color: 背景颜色
        quantize: 是否进行颜色量化
        max_colors: 最大颜色数
    
    Returns:
        输出文件路径
    """
    input_file = Path(input_path)
    print(f"\n处理文件: {input_file.name}")
    
    try:
        # 读取图片
        img = Image.open(input_path)
        original_width, original_height = img.size
        print(f"  原始尺寸: {original_width}x{original_height}")
        print(f"  模式: {img.mode}")
        
        # 转换为 RGBA 以支持透明度
        if img.mode != 'RGBA':
            img = img.convert('RGBA')
        
        # 根据模式处理图片
        if mode == 'resize':
            print(f"  模式: 直接缩放到 {target_size[0]}x{target_size[1]}")
            img_resized = img.resize(target_size, Image.LANCZOS)
        elif mode == 'letterbox':
            print(f"  模式: 保持宽高比（letterbox）到 {target_size[0]}x{target_size[1]}")
            img_resized = resize_with_aspect_ratio(img, target_size, maintain_aspect=True, background_color=background_color)
        elif mode == 'crop':
            print(f"  模式: 智能裁剪到 {target_size[0]}x{target_size[1]}")
            img_resized = smart_crop(img, target_size)
        else:
            raise ValueError(f"未知的缩放模式: {mode}")
        
        # 颜色量化
        if quantize:
            print(f"  颜色量化: 最大 {max_colors} 种颜色")
            img_resized = quantize_colors(img_resized, max_colors)
        
        # 确保输出目录存在
        output_dir_path = Path(output_dir)
        output_dir_path.mkdir(exist_ok=True)
        
        # 保存图片
        output_path = output_dir_path / input_file.name
        img_resized.save(output_path, 'PNG')
        print(f"  ✓ 已生成: {output_path}")
        print(f"  最终尺寸: {img_resized.size[0]}x{img_resized.size[1]}")
        
        return output_path
        
    except Exception as e:
        print(f"  ✗ 错误: {e}")
        return None


def main():
    """主函数"""
    parser = argparse.ArgumentParser(
        description='图片缩放转换器 - 将图片转换到指定大小，保持辨识度',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例:
  # 将单张图片缩放到 32x32
  python img2x_converter.py input.png -s 32x32
  
  # 批量处理文件夹中的所有图片
  python img2x_converter.py input_folder/ -s 32x32 -o output/
  
  # 使用 letterbox 模式保持宽高比
  python img2x_converter.py image.png -s 32x32 -m letterbox
  
  # 使用智能裁剪模式
  python img2x_converter.py image.png -s 32x32 -m crop
  
  # 指定背景颜色（letterbox 模式）
  python img2x_converter.py image.png -s 32x32 -m letterbox -bg 255,255,255,255
  
  # 启用颜色量化，限制为 16 种颜色（适合像素画）
  python img2x_converter.py image.png -s 32x32 --quantize --max-colors 16
        """
    )
    
    parser.add_argument('input', help='输入图片或文件夹路径')
    parser.add_argument('-s', '--size', required=True, help='目标尺寸，格式: WIDTHxHEIGHT (如 32x32)')
    parser.add_argument('-o', '--output', default='resized', help='输出目录 (默认: resized)')
    parser.add_argument('-m', '--mode', choices=['resize', 'letterbox', 'crop'],
                        default='resize', help='缩放模式: resize(直接缩放), letterbox(保持宽高比), crop(智能裁剪) (默认: resize)')
    parser.add_argument('-bg', '--background', default='255,255,255,255',
                        help='背景颜色 (letterbox 模式)，格式: R,G,B,A (默认: 255,255,255,255)')
    parser.add_argument('--quantize', action='store_true', help='启用颜色量化（减少颜色数）')
    parser.add_argument('--max-colors', type=int, default=256,
                        help='最大颜色数（启用量化时有效，默认: 256）')
    
    args = parser.parse_args()
    
    # 解析目标尺寸
    try:
        size_parts = args.size.lower().split('x')
        if len(size_parts) != 2:
            raise ValueError
        target_size = (int(size_parts[0]), int(size_parts[1]))
    except ValueError:
        print(f"✗ 错误: 无效的尺寸格式 '{args.size}'，请使用 WIDTHxHEIGHT 格式 (如 32x32)")
        return
    
    # 解析背景颜色
    try:
        bg_parts = [int(x.strip()) for x in args.background.split(',')]
        if len(bg_parts) == 3:
            background_color = (*bg_parts, 255)
        elif len(bg_parts) == 4:
            background_color = tuple(bg_parts)
        else:
            raise ValueError
    except ValueError:
        print(f"✗ 错误: 无效的背景颜色格式 '{args.background}'，请使用 R,G,B,A 格式 (如 255,255,255,255)")
        return
    
    # 设置路径
    input_path = Path(args.input)
    output_dir = Path(args.output)
    
    # 检查输入路径
    if not input_path.exists():
        print(f"✗ 错误: 输入路径不存在: {input_path}")
        return
    
    # 确定要处理的文件
    if input_path.is_file():
        # 单个文件
        files = [input_path]
    elif input_path.is_dir():
        # 文件夹中的所有图片文件
        supported_formats = {'.png', '.jpg', '.jpeg', '.gif', '.bmp', '.webp', '.tiff'}
        files = [f for f in input_path.iterdir() if f.is_file() and f.suffix.lower() in supported_formats]
    else:
        print(f"✗ 错误: 无效的输入路径: {input_path}")
        return
    
    if not files:
        print(f"在 {input_path} 中未找到支持的图片文件")
        return
    
    print(f"找到 {len(files)} 个图片文件")
    print(f"目标尺寸: {target_size[0]}x{target_size[1]}")
    print(f"缩放模式: {args.mode}")
    if args.mode == 'letterbox':
        print(f"背景颜色: {background_color}")
    if args.quantize:
        print(f"颜色量化: 最大 {args.max_colors} 种颜色")
    
    # 处理每个文件
    success_count = 0
    for file_path in files:
        result = process_image(file_path, output_dir, target_size, args.mode, background_color, args.quantize, args.max_colors)
        if result:
            success_count += 1
    
    print(f"\n转换完成！")
    print(f"成功: {success_count}/{len(files)}")
    print(f"输出目录: {output_dir.resolve()}")


if __name__ == '__main__':
    main()