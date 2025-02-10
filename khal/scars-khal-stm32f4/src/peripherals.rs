pub use stm32f4xx_hal::pac::{self, *};

#[unsafe(no_mangle)]
static mut NON_KERNEL_PERIPHERALS: bool = false;
#[doc = r"All the peripherals not used by the kernel"]
#[allow(non_snake_case)]
pub struct Peripherals {
    #[doc = "RNG"]
    pub RNG: RNG,
    #[doc = "HASH"]
    pub HASH: HASH,
    #[doc = "CRYP"]
    pub CRYP: CRYP,
    #[doc = "DCMI"]
    pub DCMI: DCMI,
    #[doc = "FMC"]
    pub FMC: FMC,
    #[doc = "DBGMCU"]
    pub DBGMCU: DBGMCU,
    #[doc = "DMA2"]
    pub DMA2: DMA2,
    #[doc = "DMA1"]
    pub DMA1: DMA1,
    //#[doc = "RCC"]
    //pub RCC: RCC,
    #[doc = "GPIOK"]
    pub GPIOK: GPIOK,
    #[doc = "GPIOJ"]
    pub GPIOJ: GPIOJ,
    #[doc = "GPIOI"]
    pub GPIOI: GPIOI,
    #[doc = "GPIOH"]
    pub GPIOH: GPIOH,
    #[doc = "GPIOG"]
    pub GPIOG: GPIOG,
    #[doc = "GPIOF"]
    pub GPIOF: GPIOF,
    #[doc = "GPIOE"]
    pub GPIOE: GPIOE,
    #[doc = "GPIOD"]
    pub GPIOD: GPIOD,
    #[doc = "GPIOC"]
    pub GPIOC: GPIOC,
    #[doc = "GPIOB"]
    pub GPIOB: GPIOB,
    #[doc = "GPIOA"]
    pub GPIOA: GPIOA,
    #[doc = "SYSCFG"]
    pub SYSCFG: SYSCFG,
    #[doc = "SPI1"]
    pub SPI1: SPI1,
    #[doc = "SPI2"]
    pub SPI2: SPI2,
    #[doc = "SPI3"]
    pub SPI3: SPI3,
    #[doc = "I2S2EXT"]
    pub I2S2EXT: I2S2EXT,
    #[doc = "I2S3EXT"]
    pub I2S3EXT: I2S3EXT,
    #[doc = "SPI4"]
    pub SPI4: SPI4,
    #[doc = "SPI5"]
    pub SPI5: SPI5,
    #[doc = "SPI6"]
    pub SPI6: SPI6,
    #[doc = "SDIO"]
    pub SDIO: SDIO,
    #[doc = "ADC1"]
    pub ADC1: ADC1,
    #[doc = "ADC2"]
    pub ADC2: ADC2,
    #[doc = "ADC3"]
    pub ADC3: ADC3,
    #[doc = "USART6"]
    pub USART6: USART6,
    #[doc = "USART1"]
    pub USART1: USART1,
    #[doc = "USART2"]
    pub USART2: USART2,
    #[doc = "USART3"]
    pub USART3: USART3,
    #[doc = "UART7"]
    pub UART7: UART7,
    #[doc = "UART8"]
    pub UART8: UART8,
    #[doc = "DAC"]
    pub DAC: DAC,
    #[doc = "PWR"]
    pub PWR: PWR,
    #[doc = "IWDG"]
    pub IWDG: IWDG,
    #[doc = "WWDG"]
    pub WWDG: WWDG,
    #[doc = "RTC"]
    pub RTC: RTC,
    #[doc = "UART4"]
    pub UART4: UART4,
    #[doc = "UART5"]
    pub UART5: UART5,
    #[doc = "ADC_COMMON"]
    pub ADC_COMMON: ADC_COMMON,
    #[doc = "TIM1"]
    pub TIM1: TIM1,
    #[doc = "TIM8"]
    pub TIM8: TIM8,
    //#[doc = "TIM2"]
    //pub TIM2: TIM2,
    #[doc = "TIM3"]
    pub TIM3: TIM3,
    #[doc = "TIM4"]
    pub TIM4: TIM4,
    //#[doc = "TIM5"]
    //pub TIM5: TIM5,
    #[doc = "TIM9"]
    pub TIM9: TIM9,
    #[doc = "TIM12"]
    pub TIM12: TIM12,
    #[doc = "TIM10"]
    pub TIM10: TIM10,
    #[doc = "TIM13"]
    pub TIM13: TIM13,
    #[doc = "TIM14"]
    pub TIM14: TIM14,
    #[doc = "TIM11"]
    pub TIM11: TIM11,
    #[doc = "TIM6"]
    pub TIM6: TIM6,
    #[doc = "TIM7"]
    pub TIM7: TIM7,
    #[doc = "ETHERNET_MAC"]
    pub ETHERNET_MAC: ETHERNET_MAC,
    #[doc = "ETHERNET_MMC"]
    pub ETHERNET_MMC: ETHERNET_MMC,
    #[doc = "ETHERNET_PTP"]
    pub ETHERNET_PTP: ETHERNET_PTP,
    #[doc = "ETHERNET_DMA"]
    pub ETHERNET_DMA: ETHERNET_DMA,
    #[doc = "CRC"]
    pub CRC: CRC,
    #[doc = "OTG_FS_GLOBAL"]
    pub OTG_FS_GLOBAL: OTG_FS_GLOBAL,
    #[doc = "OTG_FS_HOST"]
    pub OTG_FS_HOST: OTG_FS_HOST,
    #[doc = "OTG_FS_DEVICE"]
    pub OTG_FS_DEVICE: OTG_FS_DEVICE,
    #[doc = "OTG_FS_PWRCLK"]
    pub OTG_FS_PWRCLK: OTG_FS_PWRCLK,
    #[doc = "CAN1"]
    pub CAN1: CAN1,
    #[doc = "CAN2"]
    pub CAN2: CAN2,
    #[doc = "FLASH"]
    pub FLASH: FLASH,
    #[doc = "EXTI"]
    pub EXTI: EXTI,
    #[doc = "OTG_HS_GLOBAL"]
    pub OTG_HS_GLOBAL: OTG_HS_GLOBAL,
    #[doc = "OTG_HS_HOST"]
    pub OTG_HS_HOST: OTG_HS_HOST,
    #[doc = "OTG_HS_DEVICE"]
    pub OTG_HS_DEVICE: OTG_HS_DEVICE,
    #[doc = "OTG_HS_PWRCLK"]
    pub OTG_HS_PWRCLK: OTG_HS_PWRCLK,
    #[doc = "LTDC"]
    pub LTDC: LTDC,
    #[doc = "SAI"]
    pub SAI: SAI,
    #[doc = "DMA2D"]
    pub DMA2D: DMA2D,
    #[doc = "I2C3"]
    pub I2C3: I2C3,
    #[doc = "I2C2"]
    pub I2C2: I2C2,
    #[doc = "I2C1"]
    pub I2C1: I2C1,
    #[doc = "FPU"]
    pub FPU: FPU,
    #[doc = "STK"]
    pub STK: STK,
    #[doc = "NVIC_STIR"]
    pub NVIC_STIR: NVIC_STIR,
    #[doc = "FPU_CPACR"]
    pub FPU_CPACR: FPU_CPACR,
    #[doc = "SCB_ACTRL"]
    pub SCB_ACTRL: SCB_ACTRL,
}
impl Peripherals {
    #[doc = r"Returns all the peripherals *once*"]
    #[inline]
    pub fn take() -> Option<Self> {
        cortex_m::interrupt::free(|_| {
            if unsafe { NON_KERNEL_PERIPHERALS } {
                None
            } else {
                Some(unsafe { Peripherals::steal() })
            }
        })
    }
    #[doc = r"Unchecked version of `Peripherals::take`"]
    #[inline]
    pub unsafe fn steal() -> Self {
        unsafe { NON_KERNEL_PERIPHERALS = true };
        let pp = unsafe { pac::Peripherals::steal() };
        Peripherals {
            RNG: pp.RNG,
            HASH: pp.HASH,
            CRYP: pp.CRYP,
            DCMI: pp.DCMI,
            FMC: pp.FMC,
            DBGMCU: pp.DBGMCU,
            DMA2: pp.DMA2,
            DMA1: pp.DMA1,
            //RCC: pp.RCC,
            GPIOK: pp.GPIOK,
            GPIOJ: pp.GPIOJ,
            GPIOI: pp.GPIOI,
            GPIOH: pp.GPIOH,
            GPIOG: pp.GPIOG,
            GPIOF: pp.GPIOF,
            GPIOE: pp.GPIOE,
            GPIOD: pp.GPIOD,
            GPIOC: pp.GPIOC,
            GPIOB: pp.GPIOB,
            GPIOA: pp.GPIOA,
            SYSCFG: pp.SYSCFG,
            SPI1: pp.SPI1,
            SPI2: pp.SPI2,
            SPI3: pp.SPI3,
            I2S2EXT: pp.I2S2EXT,
            I2S3EXT: pp.I2S3EXT,
            SPI4: pp.SPI4,
            SPI5: pp.SPI5,
            SPI6: pp.SPI6,
            SDIO: pp.SDIO,
            ADC1: pp.ADC1,
            ADC2: pp.ADC2,
            ADC3: pp.ADC3,
            USART6: pp.USART6,
            USART1: pp.USART1,
            USART2: pp.USART2,
            USART3: pp.USART3,
            UART7: pp.UART7,
            UART8: pp.UART8,
            DAC: pp.DAC,
            PWR: pp.PWR,
            IWDG: pp.IWDG,
            WWDG: pp.WWDG,
            RTC: pp.RTC,
            UART4: pp.UART4,
            UART5: pp.UART5,
            ADC_COMMON: pp.ADC_COMMON,
            TIM1: pp.TIM1,
            TIM8: pp.TIM8,
            //TIM2: pp.TIM2,
            TIM3: pp.TIM3,
            TIM4: pp.TIM4,
            //TIM5: pp.TIM5,
            TIM9: pp.TIM9,
            TIM12: pp.TIM12,
            TIM10: pp.TIM10,
            TIM13: pp.TIM13,
            TIM14: pp.TIM14,
            TIM11: pp.TIM11,
            TIM6: pp.TIM6,
            TIM7: pp.TIM7,
            ETHERNET_MAC: pp.ETHERNET_MAC,
            ETHERNET_MMC: pp.ETHERNET_MMC,
            ETHERNET_PTP: pp.ETHERNET_PTP,
            ETHERNET_DMA: pp.ETHERNET_DMA,
            CRC: pp.CRC,
            OTG_FS_GLOBAL: pp.OTG_FS_GLOBAL,
            OTG_FS_HOST: pp.OTG_FS_HOST,
            OTG_FS_DEVICE: pp.OTG_FS_DEVICE,
            OTG_FS_PWRCLK: pp.OTG_FS_PWRCLK,
            CAN1: pp.CAN1,
            CAN2: pp.CAN2,
            FLASH: pp.FLASH,
            EXTI: pp.EXTI,
            OTG_HS_GLOBAL: pp.OTG_HS_GLOBAL,
            OTG_HS_HOST: pp.OTG_HS_HOST,
            OTG_HS_DEVICE: pp.OTG_HS_DEVICE,
            OTG_HS_PWRCLK: pp.OTG_HS_PWRCLK,
            LTDC: pp.LTDC,
            SAI: pp.SAI,
            DMA2D: pp.DMA2D,
            I2C3: pp.I2C3,
            I2C2: pp.I2C2,
            I2C1: pp.I2C1,
            FPU: pp.FPU,
            STK: pp.STK,
            NVIC_STIR: pp.NVIC_STIR,
            FPU_CPACR: pp.FPU_CPACR,
            SCB_ACTRL: pp.SCB_ACTRL,
        }
    }
}
