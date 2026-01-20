#!/usr/bin/env python3
"""
AIP 像素画转换器
将 .aip 格式的像素画文件转换为 PNG 位图
"""

import os
import re
from pathlib import Path
from PIL import Image, ImageDraw


def parse_aip_file(file_path):
    """解析 .aip 文件"""
    with open(file_path, 'r', encoding='utf-8') as f:
        content = f.read()
    
    # 提取 width 和 height
    width_match = re.search(r'width:\s*(\d+)', content)
    height_match = re.search(r'height:\s*(\d+)', content)
    
    if not width_match or not height_match:
        raise ValueError(f"文件 {file_path} 中缺少 width 或 height 定义")
    
    width = int(width_match.group(1))
    height = int(height_match.group(1))
    
    # 提取 image 矩阵
    # 找到 image: 和 color: 之间的内容
    image_start = content.find('image:')
    color_start = content.find('color:')
    
    if image_start == -1 or color_start == -1:
        raise ValueError(f"文件 {file_path} 中缺少 image 或 color 定义")
    
    image_section = content[image_start:color_start]
    # 移除 "image:" 标记
    image_section = image_section.replace('image:', '').strip()
    
    # 将矩阵转换为二维列表
    matrix = []
    all_lines = image_section.split('\n')
    for i, line in enumerate(all_lines):
        stripped = line.strip()
        if stripped:
            row = [int(x) for x in stripped.split()]
            matrix.append(row)
    
    # 检查矩阵规模是否与声明的尺寸一致
    actual_height = len(matrix)
    actual_width = len(matrix[0]) if matrix else 0
    
    if actual_height != height or actual_width != width:
        print(f"  ⚠ 警告: 声明的尺寸 ({width}x{height}) 与实际矩阵 ({actual_width}x{actual_height}) 不一致")
        
        # 调整矩阵尺寸以匹配声明的尺寸
        # 1. 调整每行的宽度
        for i in range(len(matrix)):
            if len(matrix[i]) < width:
                # 补全列（使用 0，即透明）
                matrix[i] = matrix[i] + [0] * (width - len(matrix[i]))
            elif len(matrix[i]) > width:
                # 截断列
                matrix[i] = matrix[i][:width]
        
        # 2. 调整行数
        if len(matrix) < height:
            # 补全行（使用全 0，即透明）
            for _ in range(height - len(matrix)):
                matrix.append([0] * width)
        elif len(matrix) > height:
            # 截断行
            matrix = matrix[:height]
        
        print(f"  → 已调整矩阵尺寸为: {width}x{height}")
    
    # 验证矩阵规模
    if len(matrix) != height:
        raise ValueError(f"矩阵行数 ({len(matrix)}) 与 height ({height}) 不匹配")
    
    for i, row in enumerate(matrix):
        if len(row) != width:
            raise ValueError(f"矩阵第 {i+1} 行列数 ({len(row)}) 与 width ({width}) 不匹配")
    
    # 提取 color 定义
    color_match = re.search(r'color:\s*\n((?:\d+:\s*[^\n]+\n?)*)', content)
    if not color_match:
        raise ValueError(f"文件 {file_path} 中缺少 color 定义")
    
    color_text = color_match.group(1).strip()
    colors = {}
    for line in color_text.split('\n'):
        if ':' in line:
            key, value = line.split(':', 1)
            key = key.strip()
            value = value.strip()
            if value.lower() == 'transparent':
                colors[int(key)] = None
            else:
                colors[int(key)] = value
    
    return {
        'width': width,
        'height': height,
        'matrix': matrix,
        'colors': colors,
        'filename': Path(file_path).stem
    }


def create_pixel_image(aip_data, output_dir):
    """根据 AIP 数据创建像素图"""
    width = aip_data['width']
    height = aip_data['height']
    matrix = aip_data['matrix']
    colors = aip_data['colors']
    filename = aip_data['filename']
    
    # 创建 RGBA 图像（支持透明度）
    img = Image.new('RGBA', (width, height), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    
    # 绘制每个像素
    for y in range(height):
        for x in range(width):
            pixel_value = matrix[y][x]
            if pixel_value in colors:
                color = colors[pixel_value]
                if color is not None:
                    # 解析颜色（支持十六进制格式）
                    if color.startswith('#'):
                        # 十六进制颜色
                        hex_color = color.lstrip('#')
                        if len(hex_color) == 6:
                            # RGB
                            r = int(hex_color[0:2], 16)
                            g = int(hex_color[2:4], 16)
                            b = int(hex_color[4:6], 16)
                            a = 255
                        elif len(hex_color) == 8:
                            # RGBA
                            r = int(hex_color[0:2], 16)
                            g = int(hex_color[2:4], 16)
                            b = int(hex_color[4:6], 16)
                            a = int(hex_color[6:8], 16)
                        else:
                            raise ValueError(f"无效的颜色格式: {color}")
                        draw.point((x, y), (r, g, b, a))
                    else:
                        # 其他格式，直接使用
                        draw.point((x, y), color)
    
    # 保存图像
    output_path = Path(output_dir) / f"{filename}.png"
    img.save(output_path, 'PNG')
    print(f"已生成: {output_path}")
    return output_path


def main():
    """主函数"""
    # 设置路径
    script_dir = Path(__file__).parent
    input_dir = script_dir / 'input'
    output_dir = script_dir / 'output'
    
    # 确保 output 目录存在
    output_dir.mkdir(exist_ok=True)
    
    # 查找所有 .aip 文件
    aip_files = list(input_dir.glob('*.aip'))

    if not aip_files:
        print(f"在 {input_dir} 目录中未找到 .aip 文件")
        return
    
    print(f"找到 {len(aip_files)} 个 .aip 文件")
    
    # 处理每个文件
    for aip_file in aip_files:
        print(f"\n处理文件: {aip_file.name}")
        try:
            # 解析文件
            aip_data = parse_aip_file(aip_file)
            print(f"  尺寸: {aip_data['width']}x{aip_data['height']}")
            print(f"  颜色数: {len(aip_data['colors'])}")
            
            # 验证矩阵规模
            print(f"  矩阵规模: {len(aip_data['matrix'])} 行 x {len(aip_data['matrix'][0])} 列")
            if len(aip_data['matrix']) == aip_data['height'] and len(aip_data['matrix'][0]) == aip_data['width']:
                print("  ✓ 矩阵规模验证通过")
            else:
                print("  ✗ 矩阵规模验证失败")
                continue
            
            # 创建图像
            create_pixel_image(aip_data, output_dir)
            
        except Exception as e:
            print(f"  ✗ 错误: {e}")
    
    print(f"\n转换完成！输出目录: {output_dir}")


if __name__ == '__main__':
    main()