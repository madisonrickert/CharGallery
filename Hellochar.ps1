# Navigate to the target directory
Set-Location -Path "C:\Users\LoveTech\Downloads\hellochar.com-master"

# Run 'yarn start' in a separate process (without waiting for it to return)
Write-Output "Running 'yarn start'..."
Start-Process "yarn" -ArgumentList "start"

Write-Output "Waiting for 5 seconds before opening the browser..."
Start-Sleep -Seconds 5

Write-Output "Opening browser to http://localhost:5173/cymatics..."
Start-Process "http://localhost:5173/cymatics"

# Keep the PowerShell window open
Write-Output "Script complete. Press Enter to exit."
Read-Host
