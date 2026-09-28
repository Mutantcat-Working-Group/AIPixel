#!/usr/bin/env python3
"""
AIP 像素画反向转换器
将 PNG 位图转换为 .aip 格式的像素画文件

由异猫工作群（mutantcat.org）发行 · GitHub: https://github.com/Mutantcat-Working-Group
"""

import os
from pathlib import Path
from PIL import Image


def analyze_image_colors(img):
    """分析图像中出现的所有颜色，并映射到索引"""
    # 转换为 RGBA 模式以统一处理
    img = img.convert('RGBA')
    
    width, height = img.size
    
    # 为颜色分配索引
    # 0 保留给透明
    color_to_index = {}
    index_to_color = {}
    
    # 先处理透明色
    color_to_index[(0, 0, 0, 0)] = 0
    index_to_color[0] = None
    
    # 按照颜色在图像中首次出现的顺序分配索引
    index = 1
    for y in range(height):
        for x in range(width):
            r, g, b, a = img.getpixel((x, y))
            if a == 0:
                # 透明像素
                color = (0, 0, 0, 0)
            else:
                color = (r, g, b, a)
            
            # 如果这个颜色还没有被分配索引
            if color not in color_to_index:
                color_to_index[color] = index
                if a == 0:
                    # 透明色
                    index_to_color[index] = None
                else:
                    # 非透明色
                    index_to_color[index] = f"#{r:02X}{g:02X}{b:02X}"
                index += 1
    
    return color_to_index, index_to_color


def create_aip_from_image(img, filename):
    """从图像创建 AIP 数据"""
    img = img.convert('RGBA')
    width, height = img.size
    
    # 分析颜色
    color_to_index, index_to_color = analyze_image_colors(img)
    
    # 生成像素矩阵
    matrix = []
    for y in range(height):
        row = []
        for x in range(width):
            r, g, b, a = img.getpixel((x, y))
            if a == 0:
                # 透明像素
                row.append(0)
            else:
                color = (r, g, b, a)
                row.append(color_to_index[color])
        matrix.append(row)
    
    return {
        'width': width,
        'height': height,
        'matrix': matrix,
        'colors': index_to_color,
        'filename': filename
    }


def write_aip_file(aip_data, output_dir):
    """将 AIP 数据写入文件"""
    output_path = Path(output_dir) / f"{aip_data['filename']}.aip"
    
    # 构建 AIP 文件内容
    lines = []
    lines.append(f"width: {aip_data['width']}")
    lines.append(f"height: {aip_data['height']}")
    lines.append("")
    lines.append("image:")
    
    # 写入像素矩阵
    for row in aip_data['matrix']:
        lines.append(" ".join(str(x) for x in row))
    
    lines.append("")
    lines.append("color:")
    
    # 写入颜色定义
    for index in sorted(aip_data['colors'].keys()):
        color = aip_data['colors'][index]
        if color is None:
            lines.append(f"{index}: transparent")
        else:
            lines.append(f"{index}: {color}")
    
    # 写入文件
    with open(output_path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(lines))
    
    print(f"已生成: {output_path}")
    return output_path


def main():
    """主函数"""
    # 设置路径
    script_dir = Path(__file__).parent
    output_dir = script_dir / 'refer_img'
    reverse_dir = script_dir / 'refer_aip'
    
    # 确保 reverse 目录存在
    reverse_dir.mkdir(exist_ok=True)
    
    # 查找所有 PNG 文件
    png_files = list(output_dir.glob('*.png'))
    
    if not png_files:
        print(f"在 {output_dir} 目录中未找到 PNG 文件")
        return
    
    print(f"找到 {len(png_files)} 个 PNG 文件")
    
    # 处理每个文件
    for png_file in png_files:
        print(f"\n处理文件: {png_file.name}")
        try:
            # 读取图像
            img = Image.open(png_file)
            width, height = img.size
            print(f"  尺寸: {width}x{height}")
            
            # 转换为 AIP 格式
            aip_data = create_aip_from_image(img, png_file.stem)
            print(f"  颜色数: {len(aip_data['colors'])}")
            print(f"  矩阵规模: {len(aip_data['matrix'])} 行 x {len(aip_data['matrix'][0])} 列")
            
            # 验证矩阵规模
            if len(aip_data['matrix']) == aip_data['height'] and len(aip_data['matrix'][0]) == aip_data['width']:
                print("  ✓ 矩阵规模验证通过")
            else:
                print("  ✗ 矩阵规模验证失败")
                continue
            
            # 写入 AIP 文件
            write_aip_file(aip_data, reverse_dir)
            
        except Exception as e:
            print(f"  ✗ 错误: {e}")
    
    print(f"\n转换完成！输出目录: {reverse_dir}")


if __name__ == '__main__':
    main()