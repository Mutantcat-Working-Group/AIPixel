# 由异猫工作群（mutantcat.org）发行 · GitHub: https://github.com/Mutantcat-Working-Group

from PIL import Image

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
    input_file = "/Users/tyza66/项目/AIPixel/refer_img/攻击.png"
    output_file = "/Users/tyza66/项目/AIPixel/refer_img/攻击_32x32.png"

    process_attack_to_32x32(input_file, output_file)