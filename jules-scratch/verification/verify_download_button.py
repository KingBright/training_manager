import asyncio
from playwright.async_api import async_playwright, expect

async def main():
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        page = await browser.new_page()

        await page.goto("http://127.0.0.1:3000/")

        try:
            task_item = page.locator(".task-item", has_text="My Test Task")
            await expect(task_item).to_be_visible(timeout=5000)

            download_button = task_item.get_by_role("button", name="下载输出")
            await expect(download_button).to_be_visible()

            await task_item.screenshot(path="jules-scratch/verification/verification.png")
            print("Successfully found the element and took a screenshot.")

        except AssertionError:
            print("AssertionError: The task item was not found. Dumping page content:")
            print(await page.content())

        await browser.close()

asyncio.run(main())
