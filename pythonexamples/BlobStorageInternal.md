# Azure Blob Storage Setup - Gorilla AEtherBus4

## 🎯 Overview

Your Azure Blob Storage is now configured and ready to use! This setup allows you to store and manage your gorilla_aetherbus4 logs and files in the cloud with organized structure and metadata.

## 📋 Your Storage Account Details

```bash
STORAGE_ACCOUNT="gorillabus1753798234"
RESOURCE_GROUP="Eva_Alpha"
STORAGE_KEY="t4QC8jyXQy0jAJvCWV49RlFM6zKzRZQ722t1P3eOlRVdZ537XIQZyvlPOl9s2j/xB0cZmx62HnK/+AStdYuYXA=="
CONTAINER_NAME="gorilla-logs"
```

## 🚀 Quick Commands

### Set Environment Variables (Run First)
```bash
export STORAGE_ACCOUNT="gorillabus1753798234"
export RESOURCE_GROUP="Eva_Alpha"
export STORAGE_KEY="t4QC8jyXQy0jAJvCWV49RlFM6zKzRZQ722t1P3eOlRVdZ537XIQZyvlPOl9s2j/xB0cZmx62HnK/+AStdYuYXA=="
export CONTAINER_NAME="gorilla-logs"
```

### Upload a Single File
```bash
# Upload any file with organized path structure
az storage blob upload \
    --file /path/to/your/file.log \
    --container-name $CONTAINER_NAME \
    --name "gather_agent/$(date +%Y/%m/%d)/$(basename /path/to/your/file.log)" \
    --account-name $STORAGE_ACCOUNT \
    --account-key $STORAGE_KEY \
    --metadata "agent=gather_agent" "session=tmux" "server=ag1-azure" "timestamp=$(date +%s)"
```

### Upload All Log Files
```bash
# Navigate to your logs directory first
cd ~/gorilla_aetherbus4/gorilla_aetherbus4/logs

# Upload all .log files
for logfile in *.log; do
    if [ -f "$logfile" ]; then
        echo "Uploading $logfile..."
        az storage blob upload \
            --file "$logfile" \
            --container-name $CONTAINER_NAME \
            --name "gather_agent/$(date +%Y/%m/%d)/$logfile" \
            --account-name $STORAGE_ACCOUNT \
            --account-key $STORAGE_KEY \
            --metadata "agent=gather_agent" "session=tmux" "server=ag1-azure" "upload_date=$(date +%Y-%m-%d)" \
            --overwrite
    fi
done
```

### List All Uploaded Files
```bash
# List all files in your container
az storage blob list \
    --container-name $CONTAINER_NAME \
    --account-name $STORAGE_ACCOUNT \
    --account-key $STORAGE_KEY \
    --output table
```

### Download a File
```bash
# Download a specific file
az storage blob download \
    --container-name $CONTAINER_NAME \
    --name "gather_agent/2025/07/29/your_file.log" \
    --file "./downloaded_file.log" \
    --account-name $STORAGE_ACCOUNT \
    --account-key $STORAGE_KEY
```

### Get File URLs
```bash
# Get a URL for a specific file (with SAS token for access)
az storage blob url \
    --container-name $CONTAINER_NAME \
    --name "gather_agent/2025/07/29/your_file.log" \
    --account-name $STORAGE_ACCOUNT \
    --account-key $STORAGE_KEY
```

## 📁 Organized File Structure

Your files are organized in the blob storage like this:
```
gorilla-logs/
├── gather_agent/
│   ├── 2025/
│   │   ├── 07/
│   │   │   ├── 29/
│   │   │   │   ├── gather_agent_20250729_140755.log
│   │   │   │   ├── gather_agent_20250729_150823.log
│   │   │   │   └── ...
│   │   │   └── 30/
│   │   └── 08/
│   └── ...
├── workflow_orchestrator/
│   └── 2025/07/29/...
└── other_agents/
    └── ...
```

## 🔧 Integration with Your Agents

### Add to gather_agent.py
Add this function to automatically upload logs:

```python
import os
import subprocess
from datetime import datetime

def upload_log_to_azure(log_file_path):
    """Upload log file to Azure Blob Storage"""
    try:
        storage_account = "gorillabus1753798234"
        storage_key = "t4QC8jyXQy0jAJvCWV49RlFM6zKzRZQ722t1P3eOlRVdZ537XIQZyvlPOl9s2j/xB0cZmx62HnK/+AStdYuYXA=="
        container_name = "gorilla-logs"
        
        # Create organized blob name
        date_path = datetime.now().strftime('%Y/%m/%d')
        blob_name = f"gather_agent/{date_path}/{os.path.basename(log_file_path)}"
        
        # Upload command
        cmd = [
            "az", "storage", "blob", "upload",
            "--file", log_file_path,
            "--container-name", container_name,
            "--name", blob_name,
            "--account-name", storage_account,
            "--account-key", storage_key,
            "--metadata", f"agent=gather_agent",
            "--metadata", f"server=ag1-azure",
            "--metadata", f"timestamp={int(datetime.now().timestamp())}",
            "--overwrite"
        ]
        
        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode == 0:
            print(f"✅ Uploaded {log_file_path} to Azure Blob Storage")
            return f"https://{storage_account}.blob.core.windows.net/{container_name}/{blob_name}"
        else:
            print(f"❌ Failed to upload {log_file_path}: {result.stderr}")
            return None
            
    except Exception as e:
        print(f"❌ Error uploading to Azure: {e}")
        return None

# Usage in your agent
# upload_log_to_azure("logs/gather_agent_20250729.log")
```

## 🔍 Monitoring and Management

### Check Storage Usage
```bash
# Get storage account information
az storage account show \
    --name $STORAGE_ACCOUNT \
    --resource-group $RESOURCE_GROUP \
    --query '{name:name,location:location,sku:sku.name,accessTier:accessTier}' \
    --output table
```

### Search Files by Metadata
```bash
# Find files by agent type
az storage blob list \
    --container-name $CONTAINER_NAME \
    --account-name $STORAGE_ACCOUNT \
    --account-key $STORAGE_KEY \
    --query "[?metadata.agent=='gather_agent']" \
    --output table
```

### Clean Up Old Files
```bash
# Delete files older than 30 days (be careful!)
az storage blob list \
    --container-name $CONTAINER_NAME \
    --account-name $STORAGE_ACCOUNT \
    --account-key $STORAGE_KEY \
    --query "[?properties.lastModified<'$(date -d '30 days ago' --iso-8601)'].name" \
    --output tsv | while read blob; do
    echo "Deleting old file: $blob"
    az storage blob delete \
        --container-name $CONTAINER_NAME \
        --name "$blob" \
        --account-name $STORAGE_ACCOUNT \
        --account-key $STORAGE_KEY
done
```

## 🛡️ Security Notes

1. **Keep your storage key secure** - It's like a password for your storage account
2. **The storage key is in this README** - Consider moving it to environment variables
3. **Container is private** - Files are not publicly accessible without the key
4. **Rotate keys periodically** - You can regenerate keys in Azure portal

## 🚨 Troubleshooting

### Common Issues

1. **Authentication Failed**
   ```bash
   # Verify your credentials
   az storage account show --name $STORAGE_ACCOUNT --resource-group $RESOURCE_GROUP
   ```

2. **File Not Found**
   ```bash
   # Check if file exists locally
   ls -la /path/to/your/file.log
   ```

3. **Container Not Found**
   ```bash
   # List containers
   az storage container list --account-name $STORAGE_ACCOUNT --account-key $STORAGE_KEY
   ```

4. **Permission Denied**
   ```bash
   # Check Azure login status
   az account show
   ```

## 🎯 Next Steps

1. **Automate uploads** - Add the upload function to your agents
2. **Set up monitoring** - Create alerts for storage usage
3. **Backup strategy** - Consider geo-redundant storage for important logs
4. **Cost optimization** - Move old files to cheaper storage tiers

## 📞 Support Commands

```bash
# Get help for any az storage command
az storage blob --help
az storage container --help
az storage account --help
```

Your Azure Blob Storage is ready to use! Start uploading your logs and enjoy organized, scalable cloud storage for your gorilla_aetherbus4 project.
